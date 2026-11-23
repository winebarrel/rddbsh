//! The WHERE-clause wrapper with the GetItem/Query/Scan decision helpers
//! (port of expressions/where.hpp).

use super::logical::{KeyMap, KeySchemaView, LogicalExpr};
use super::symbol_table::SymbolTable;
use crate::value::Value;

#[derive(Debug, Clone, PartialEq)]
pub struct QueryKeyPlan {
    pub pk: Vec<String>,
    pub sk_prefix: Vec<String>,
}

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

    pub fn plan_query(&self, pk: &[String], rk: &[String]) -> Option<QueryKeyPlan> {
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
        let view = KeySchemaView { pk, sk: &prefix };
        if !self.root.query_safe(view) {
            return None;
        }
        Some(QueryKeyPlan {
            pk: pk.to_vec(),
            sk_prefix: prefix,
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

    pub fn query_key_condition_expression(
        &self,
        plan: &QueryKeyPlan,
        st: Option<&mut SymbolTable>,
    ) -> String {
        let view = KeySchemaView {
            pk: &plan.pk,
            sk: &plan.sk_prefix,
        };
        let mut st = st;
        self.root.query_key_condition_expression(view, &mut st)
    }

    pub fn query_filter_expression(
        &self,
        plan: &QueryKeyPlan,
        st: Option<&mut SymbolTable>,
    ) -> String {
        let view = KeySchemaView {
            pk: &plan.pk,
            sk: &plan.sk_prefix,
        };
        let mut st = st;
        self.root.query_filter_expression(view, &mut st)
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
