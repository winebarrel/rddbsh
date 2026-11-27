// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! The WHERE-clause wrapper with the GetItem/Query/Scan decision helpers
//! (port of expressions/where.hpp).

use std::collections::BTreeMap;

use super::logical::{KeyMap, KeySchemaView, LogicalExpr, NeLeg};
use super::symbol_table::SymbolTable;
use crate::table_schema::ScalarType;
use crate::value::Value;

/// The key attributes a Query uses. `ne_split` is set when the clause's only
/// predicate on the last prefix attribute is a not-equal: the plan is then
/// two Queries, the lower leg (`#sk < :v`) and the upper leg (`#sk > :v`),
/// with the same filter.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryKeyPlan {
    pub pk: Vec<String>,
    pub sk_prefix: Vec<String>,
    pub ne_split: Option<String>,
}

impl QueryKeyPlan {
    fn view(&self, leg: NeLeg) -> KeySchemaView<'_> {
        KeySchemaView {
            pk: &self.pk,
            sk: &self.sk_prefix,
            ne_split: self.ne_split.as_deref(),
            leg,
        }
    }
}

/// Declared scalar types of key attributes (AttributeDefinitions), by name.
pub type KeyTypes = BTreeMap<String, ScalarType>;

#[derive(Debug, Clone, PartialEq)]
pub struct Where {
    root: LogicalExpr,
}

impl Where {
    pub fn new(root: LogicalExpr) -> Where {
        Where { root }
    }

    pub fn root(&self) -> &LogicalExpr {
        &self.root
    }

    pub fn can_getitem(&self, pk: &[String], rk: &[String]) -> bool {
        if pk.len() != 1 {
            return false;
        }
        if !rk.is_empty() {
            return self.root.count() == 2
                && self.root.count_gud_key(&pk[0]) == 1
                && self.root.count_gud_key(&rk[0]) == 1;
        }
        self.root.count() == 1 && self.root.count_gud_key(&pk[0]) == 1
    }

    /// When the clause is not query-safe only because of a not-equal on the
    /// last prefix attribute, the plan splits it into two Queries (see
    /// `split_on_not_equal`). `key_types` are the declared key types from
    /// DescribeTable (`None` or missing entries: unknown).
    pub fn plan_query(
        &self,
        pk: &[String],
        rk: &[String],
        key_types: Option<&KeyTypes>,
    ) -> Option<QueryKeyPlan> {
        for p in pk {
            if !(self.root.count_gud_key(p) == 1 && self.root.count_lhs(p) == 1) {
                return None;
            }
        }
        let mut prefix: Vec<String> = Vec::new();
        let mut ended = false;
        for s in rk {
            if self.root.count_lhs(s) == 0 {
                ended = true;
                continue;
            }
            if ended {
                return None;
            }
            prefix.push(s.clone());
        }
        if prefix.len() > 1 {
            for s in &prefix[..prefix.len() - 1] {
                if !(self.root.count_lhs(s) == 1 && self.root.count_gud_key(s) == 1) {
                    return None;
                }
            }
        }
        if self.root.query_safe(KeySchemaView::new(pk, &prefix)) {
            return Some(QueryKeyPlan {
                pk: pk.to_vec(),
                sk_prefix: prefix,
                ne_split: None,
            });
        }
        // Or query-safe once a not-equal on the last prefix attribute is split
        // into two Queries.
        self.split_on_not_equal(pk, prefix, key_types)
    }

    /// A not-equal on the last sort-key prefix attribute `s` plans as two
    /// Queries (`s < v`, then `s > v`) when it is the only predicate on `s`,
    /// a conjunct of the top-level AND chain, and its value is a scalar of
    /// the key's type (a mistyped literal would make the key condition a
    /// ValidationException, so it stays a Scan). Every other predicate must
    /// be query-safe as before.
    fn split_on_not_equal(
        &self,
        pk: &[String],
        prefix: Vec<String>,
        key_types: Option<&KeyTypes>,
    ) -> Option<QueryKeyPlan> {
        let s = prefix.last()?;
        if self.root.count_lhs(s) != 1 {
            return None;
        }
        let v = self.root.ne_key_value(s)?;
        if !key_typed_scalar(v, s, key_types) {
            return None;
        }
        let view = KeySchemaView {
            pk,
            sk: &prefix,
            ne_split: Some(s),
            leg: NeLeg::Lower,
        };
        if !self.root.query_safe(view) {
            return None;
        }
        let s = s.clone();
        Some(QueryKeyPlan {
            pk: pk.to_vec(),
            sk_prefix: prefix,
            ne_split: Some(s),
        })
    }

    pub fn can_update_item(&self, pk: &[String], rk: &[String]) -> bool {
        if pk.len() != 1 {
            return false;
        }
        if !rk.is_empty() {
            return self.root.count_gud_key(&pk[0]) == 1 && self.root.count_gud_key(&rk[0]) == 1;
        }
        self.root.count_gud_key(&pk[0]) == 1
    }

    pub fn can_delete_item(&self, pk: &[String], rk: &[String]) -> bool {
        self.can_update_item(pk, rk)
    }

    pub fn get_gud_key(&self, pk: &[String], rk: &[String]) -> KeyMap {
        let p = pk.first().map(String::as_str).unwrap_or("");
        let r = rk.first().map(String::as_str).unwrap_or("");
        self.root.get_gud_key(p, r)
    }

    /// The KeyConditionExpression for `plan`. For a not-equal split, `leg`
    /// picks the side of the value (lower: `<`, upper: `>`); otherwise it is
    /// ignored.
    pub fn query_key_condition_expression(
        &self,
        plan: &QueryKeyPlan,
        st: Option<&mut SymbolTable>,
        leg: NeLeg,
    ) -> String {
        let mut st = st;
        self.root
            .query_key_condition_expression(plan.view(leg), &mut st)
    }

    /// The FilterExpression for `plan` (the same for both legs of a split).
    pub fn query_filter_expression(
        &self,
        plan: &QueryKeyPlan,
        st: Option<&mut SymbolTable>,
    ) -> String {
        let mut st = st;
        self.root
            .query_filter_expression(plan.view(NeLeg::Lower), &mut st)
    }

    pub fn scan_filter_expression(&self, st: Option<&mut SymbolTable>) -> String {
        let mut st = st;
        self.root.serialize(&mut st)
    }

    pub fn update_delete_condition_check(
        &self,
        pk: &str,
        rk: &str,
        st: Option<&mut SymbolTable>,
    ) -> String {
        let mut st = st;
        self.root
            .update_delete_condition_check(pk, rk, &mut st, true)
    }

    pub fn collect_param_slots<'a>(&'a mut self, out: &mut Vec<&'a mut Value>) {
        self.root.collect_param_slots(out);
    }

    pub fn count_params(&self) -> usize {
        self.root.count_params()
    }
}

/// Whether `v` can stand in a key condition on `name`: a resolved S, N or B,
/// of the declared type when DescribeTable reported one.
fn key_typed_scalar(v: &Value, name: &str, key_types: Option<&KeyTypes>) -> bool {
    let ty = match v {
        Value::S(_) => ScalarType::S,
        Value::N(_) => ScalarType::N,
        Value::B(_) => ScalarType::B,
        // An unbound marker is not a value yet; any other type is not a key.
        _ => return false,
    };
    match key_types.and_then(|t| t.get(name)) {
        Some(declared) => *declared == ty,
        None => true,
    }
}
