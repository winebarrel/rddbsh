// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! The WHERE logical-expression tree (port of expressions/logical_*.hpp).
//!
//! Each node carries `negate` (rendered as `NOT (...)`) and `parens`
//! (an explicit parenthesized group, which blocks AND/OR flattening).

use std::collections::BTreeMap;

use super::attribute_serialize::serialize_attribute;
use super::symbol_table::SymbolTable;
use crate::value::Value;

pub type KeyMap = BTreeMap<String, Value>;

#[derive(Debug, Clone, Copy)]
pub struct KeySchemaView<'a> {
    pub pk: &'a [String],
    pub sk: &'a [String],
}

fn key_contains(list: &[String], name: &str) -> bool {
    list.iter().any(|e| e == name)
}

#[derive(Debug, Clone, PartialEq)]
pub enum LogicalKind {
    And(Vec<LogicalExpr>),
    Or(Vec<LogicalExpr>),
    /// `lhs op rhs`; `!=` is stored as `=` + negate.
    Comparison {
        lhs: String,
        op: String,
        rhs: Value,
    },
    In {
        lhs: String,
        rhs: Vec<Value>,
    },
    Contains {
        lhs: String,
        operand: Value,
    },
    AttributeType {
        lhs: String,
        ty: String,
    },
    AttributeExists {
        lhs: String,
    },
    Between {
        lhs: String,
        lo: Value,
        hi: Value,
    },
    BeginsWith {
        lhs: String,
        prefix: Value,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct LogicalExpr {
    pub negate: bool,
    pub parens: bool,
    pub kind: LogicalKind,
}

type St<'a> = Option<&'a mut SymbolTable>;

fn attr(st: &mut St, name: &str) -> String {
    match st {
        Some(t) => t.new_attribute(name),
        None => name.to_string(),
    }
}
fn val(st: &mut St, v: &Value) -> String {
    match st {
        Some(t) => t.new_value(v.clone()),
        None => serialize_attribute(v),
    }
}
fn val_quoted(st: &mut St, v: &Value) -> String {
    match st {
        Some(t) => t.new_value(v.clone()),
        None => format!("\"{}\"", serialize_attribute(v)),
    }
}

impl LogicalExpr {
    fn leaf(kind: LogicalKind) -> LogicalExpr {
        LogicalExpr {
            negate: false,
            parens: false,
            kind,
        }
    }

    /// `!=` desugars to a negated `=`; `<>` is passed through unchanged, as
    /// in the original.
    pub fn comparison(lhs: impl Into<String>, op: &str, rhs: Value) -> LogicalExpr {
        if op == "!=" {
            let mut e = Self::leaf(LogicalKind::Comparison {
                lhs: lhs.into(),
                op: "=".into(),
                rhs,
            });
            e.negate = true;
            e
        } else {
            Self::leaf(LogicalKind::Comparison {
                lhs: lhs.into(),
                op: op.into(),
                rhs,
            })
        }
    }
    pub fn in_list(lhs: impl Into<String>, rhs: Vec<Value>) -> LogicalExpr {
        Self::leaf(LogicalKind::In {
            lhs: lhs.into(),
            rhs,
        })
    }
    pub fn contains(lhs: impl Into<String>, operand: Value) -> LogicalExpr {
        Self::leaf(LogicalKind::Contains {
            lhs: lhs.into(),
            operand,
        })
    }
    pub fn attribute_type(lhs: impl Into<String>, ty: impl Into<String>) -> LogicalExpr {
        Self::leaf(LogicalKind::AttributeType {
            lhs: lhs.into(),
            ty: ty.into(),
        })
    }
    /// `attribute_exists(lhs)` when `sense`, else `NOT (attribute_exists(lhs))`.
    pub fn attribute_exists(lhs: impl Into<String>, sense: bool) -> LogicalExpr {
        let mut e = Self::leaf(LogicalKind::AttributeExists { lhs: lhs.into() });
        e.negate = !sense;
        e
    }
    pub fn between(lhs: impl Into<String>, lo: Value, hi: Value) -> LogicalExpr {
        Self::leaf(LogicalKind::Between {
            lhs: lhs.into(),
            lo,
            hi,
        })
    }
    pub fn begins_with(lhs: impl Into<String>, prefix: Value) -> LogicalExpr {
        Self::leaf(LogicalKind::BeginsWith {
            lhs: lhs.into(),
            prefix,
        })
    }

    pub fn negate(&mut self) {
        self.negate = !self.negate;
    }
    pub fn set_paren_group(&mut self) {
        self.parens = true;
    }

    /// AND-combine with the original's flattening rules.
    pub fn make_and(mut e1: LogicalExpr, e2: LogicalExpr) -> LogicalExpr {
        let e1_and = matches!(e1.kind, LogicalKind::And(_));
        let e2_and = matches!(e2.kind, LogicalKind::And(_));
        if e1.negate == e2.negate && e1_and && e2_and && !e1.parens && !e2.parens {
            if let (LogicalKind::And(l1), LogicalKind::And(l2)) = (&mut e1.kind, e2.kind) {
                l1.extend(l2);
            }
            return e1;
        }
        if !e1.negate && e1_and {
            if let LogicalKind::And(l1) = &mut e1.kind {
                l1.push(e2);
            }
            return e1;
        }
        Self::leaf(LogicalKind::And(vec![e1, e2]))
    }

    /// OR-combine with the original's flattening rules.
    pub fn make_or(mut e1: LogicalExpr, e2: LogicalExpr) -> LogicalExpr {
        let e1_or = matches!(e1.kind, LogicalKind::Or(_));
        let e2_or = matches!(e2.kind, LogicalKind::Or(_));
        if e1.negate == e2.negate && e1_or && e2_or && !e1.parens && !e2.parens {
            if let (LogicalKind::Or(l1), LogicalKind::Or(l2)) = (&mut e1.kind, e2.kind) {
                l1.extend(l2);
            }
            return e1;
        }
        if !e1.negate && e1_or {
            if let LogicalKind::Or(l1) = &mut e1.kind {
                l1.push(e2);
            }
            return e1;
        }
        Self::leaf(LogicalKind::Or(vec![e1, e2]))
    }

    fn lhs(&self) -> Option<&str> {
        match &self.kind {
            LogicalKind::And(_) | LogicalKind::Or(_) => None,
            LogicalKind::Comparison { lhs, .. }
            | LogicalKind::In { lhs, .. }
            | LogicalKind::Contains { lhs, .. }
            | LogicalKind::AttributeType { lhs, .. }
            | LogicalKind::AttributeExists { lhs }
            | LogicalKind::Between { lhs, .. }
            | LogicalKind::BeginsWith { lhs, .. } => Some(lhs),
        }
    }

    // ---- counting -----------------------------------------------------

    pub fn count(&self) -> i32 {
        match &self.kind {
            LogicalKind::And(l) | LogicalKind::Or(l) => l.iter().map(|c| c.count()).sum(),
            _ => 1,
        }
    }

    pub fn count_lhs(&self, name: &str) -> i32 {
        match &self.kind {
            LogicalKind::And(l) | LogicalKind::Or(l) => l.iter().map(|c| c.count_lhs(name)).sum(),
            _ => (self.lhs() == Some(name)) as i32,
        }
    }

    pub fn count_gud_key(&self, name: &str) -> i32 {
        match &self.kind {
            LogicalKind::And(l) => l.iter().map(|c| c.count_gud_key(name)).sum(),
            LogicalKind::Comparison { lhs, op, .. } => {
                (lhs == name && !self.negate && op == "=") as i32
            }
            _ => 0,
        }
    }

    pub fn get_gud_key(&self, pk: &str, rk: &str) -> KeyMap {
        match &self.kind {
            LogicalKind::And(l) => {
                let mut rv = KeyMap::new();
                for child in l {
                    for (k, v) in child.get_gud_key(pk, rk) {
                        rv.entry(k).or_insert(v);
                    }
                }
                rv
            }
            LogicalKind::Comparison { lhs, rhs, .. } => {
                let mut rv = KeyMap::new();
                if self.count_gud_key(pk) != 0 || self.count_gud_key(rk) != 0 {
                    rv.insert(lhs.clone(), rhs.clone());
                }
                rv
            }
            _ => KeyMap::new(),
        }
    }

    // ---- parameters ---------------------------------------------------

    pub fn collect_param_slots<'a>(&'a mut self, out: &mut Vec<&'a mut Value>) {
        match &mut self.kind {
            LogicalKind::And(l) | LogicalKind::Or(l) => {
                l.iter_mut().for_each(|c| c.collect_param_slots(out))
            }
            LogicalKind::Comparison { rhs, .. } => rhs.collect_param_slots(out),
            LogicalKind::In { rhs, .. } => rhs.iter_mut().for_each(|v| v.collect_param_slots(out)),
            LogicalKind::Contains { operand, .. } => operand.collect_param_slots(out),
            LogicalKind::AttributeType { .. } | LogicalKind::AttributeExists { .. } => {}
            LogicalKind::Between { lo, hi, .. } => {
                lo.collect_param_slots(out);
                hi.collect_param_slots(out);
            }
            LogicalKind::BeginsWith { prefix, .. } => prefix.collect_param_slots(out),
        }
    }

    pub fn count_params(&self) -> usize {
        match &self.kind {
            LogicalKind::And(l) | LogicalKind::Or(l) => l.iter().map(|c| c.count_params()).sum(),
            LogicalKind::Comparison { rhs, .. } => rhs.count_params(),
            LogicalKind::In { rhs, .. } => rhs.iter().map(Value::count_params).sum(),
            LogicalKind::Contains { operand, .. } => operand.count_params(),
            LogicalKind::AttributeType { .. } | LogicalKind::AttributeExists { .. } => 0,
            LogicalKind::Between { lo, hi, .. } => lo.count_params() + hi.count_params(),
            LogicalKind::BeginsWith { prefix, .. } => prefix.count_params(),
        }
    }

    // ---- serialization ------------------------------------------------

    fn wrap_negate(&self, rv: String) -> String {
        if self.negate {
            format!("NOT ({})", rv)
        } else {
            rv
        }
    }

    pub fn serialize(&self, st: &mut St) -> String {
        let rv = self.do_serialize(st);
        self.wrap_negate(rv)
    }

    fn do_serialize(&self, st: &mut St) -> String {
        match &self.kind {
            LogicalKind::And(l) | LogicalKind::Or(l) => {
                let sep = if matches!(self.kind, LogicalKind::And(_)) {
                    " AND "
                } else {
                    " OR "
                };
                let parts: Vec<String> = l.iter().map(|c| c.serialize(st)).collect();
                let body = parts.join(sep);
                if self.parens {
                    format!("({})", body)
                } else {
                    body
                }
            }
            LogicalKind::Comparison { lhs, op, rhs } => {
                let l = attr(st, lhs);
                let r = val(st, rhs);
                format!("{} {} {}", l, op, r)
            }
            LogicalKind::In { lhs, rhs } => {
                let mut rv = format!("{} IN (", attr(st, lhs));
                let parts: Vec<String> = rhs.iter().map(|v| val(st, v)).collect();
                rv.push_str(&parts.join(", "));
                rv.push(')');
                rv
            }
            LogicalKind::Contains { lhs, operand } => {
                let l = attr(st, lhs);
                let r = val_quoted(st, operand);
                format!("contains({}, {})", l, r)
            }
            LogicalKind::AttributeType { lhs, ty } => {
                let l = attr(st, lhs);
                let t = match st {
                    Some(t) => t.new_value(Value::S(ty.clone())),
                    None => ty.clone(),
                };
                format!("attribute_type({}, {})", l, t)
            }
            LogicalKind::AttributeExists { lhs } => format!("attribute_exists({})", attr(st, lhs)),
            LogicalKind::Between { lhs, lo, hi } => {
                let l = attr(st, lhs);
                let a = val(st, lo);
                let b = val(st, hi);
                format!("{} BETWEEN {} AND {}", l, a, b)
            }
            LogicalKind::BeginsWith { lhs, prefix } => {
                let l = attr(st, lhs);
                let r = val_quoted(st, prefix);
                format!("begins_with({}, {})", l, r)
            }
        }
    }

    fn join_children(
        &self,
        list: &[LogicalExpr],
        sep: &str,
        f: impl FnMut(&LogicalExpr) -> String,
    ) -> String {
        let parts: Vec<String> = list.iter().map(f).filter(|p| !p.is_empty()).collect();
        let rv = parts.join(sep);
        if self.parens {
            format!("({})", rv)
        } else {
            rv
        }
    }

    pub fn query_key_condition_expression(&self, ks: KeySchemaView, st: &mut St) -> String {
        let rv = self.do_query_kce(ks, st);
        if rv.is_empty() {
            rv
        } else {
            self.wrap_negate(rv)
        }
    }

    fn do_query_kce(&self, ks: KeySchemaView, st: &mut St) -> String {
        match &self.kind {
            LogicalKind::And(l) => {
                self.join_children(l, " AND ", |c| c.query_key_condition_expression(ks, st))
            }
            LogicalKind::Or(_) => String::new(),
            LogicalKind::Comparison { lhs, .. } => {
                if key_contains(ks.pk, lhs) || key_contains(ks.sk, lhs) {
                    self.do_serialize(st)
                } else {
                    String::new()
                }
            }
            LogicalKind::Between { lhs, .. } | LogicalKind::BeginsWith { lhs, .. } => {
                if key_contains(ks.sk, lhs) && !self.negate {
                    self.do_serialize(st)
                } else {
                    String::new()
                }
            }
            _ => String::new(),
        }
    }

    pub fn query_filter_expression(&self, ks: KeySchemaView, st: &mut St) -> String {
        let rv = self.do_query_filter(ks, st);
        if rv.is_empty() {
            rv
        } else {
            self.wrap_negate(rv)
        }
    }

    fn do_query_filter(&self, ks: KeySchemaView, st: &mut St) -> String {
        match &self.kind {
            LogicalKind::And(l) => {
                self.join_children(l, " AND ", |c| c.query_filter_expression(ks, st))
            }
            LogicalKind::Or(_) => self.do_serialize(st),
            LogicalKind::Comparison { lhs, .. } => {
                if !key_contains(ks.pk, lhs) && !key_contains(ks.sk, lhs) {
                    self.do_serialize(st)
                } else {
                    String::new()
                }
            }
            LogicalKind::Between { lhs, .. } | LogicalKind::BeginsWith { lhs, .. } => {
                if !key_contains(ks.sk, lhs) {
                    self.do_serialize(st)
                } else {
                    String::new()
                }
            }
            _ => self.do_serialize(st),
        }
    }

    pub fn update_delete_condition_check(
        &self,
        pk: &str,
        rk: &str,
        st: &mut St,
        top: bool,
    ) -> String {
        let rv = self.do_udcc(pk, rk, st, top);
        if rv.is_empty() {
            rv
        } else {
            self.wrap_negate(rv)
        }
    }

    fn do_udcc(&self, pk: &str, rk: &str, st: &mut St, top: bool) -> String {
        match &self.kind {
            LogicalKind::And(l) => self.join_children(l, " AND ", |c| {
                c.update_delete_condition_check(pk, rk, st, top)
            }),
            LogicalKind::Or(l) => self.join_children(l, " OR ", |c| {
                c.update_delete_condition_check(pk, rk, st, false)
            }),
            LogicalKind::Comparison { lhs, .. }
            | LogicalKind::Between { lhs, .. }
            | LogicalKind::BeginsWith { lhs, .. } => {
                if !top {
                    return self.do_serialize(st);
                }
                if (lhs == pk || lhs == rk) && !self.negate {
                    return String::new();
                }
                self.do_serialize(st)
            }
            _ => self.do_serialize(st),
        }
    }

    pub fn query_safe(&self, ks: KeySchemaView) -> bool {
        match &self.kind {
            LogicalKind::And(l) => l.iter().all(|c| c.query_safe(ks)),
            LogicalKind::Or(l) => {
                if ks.pk.iter().any(|p| self.count_lhs(p) > 0)
                    || ks.sk.iter().any(|s| self.count_lhs(s) > 0)
                {
                    return false;
                }
                l.iter().all(|c| c.query_safe(ks))
            }
            LogicalKind::Comparison { lhs, op, .. } => {
                if !key_contains(ks.pk, lhs) && !key_contains(ks.sk, lhs) {
                    return true;
                }
                if key_contains(ks.pk, lhs) {
                    return op == "=" && !self.negate;
                }
                !(op == "=" && self.negate)
            }
            LogicalKind::Between { lhs, .. } | LogicalKind::BeginsWith { lhs, .. } => {
                if !key_contains(ks.pk, lhs) && !key_contains(ks.sk, lhs) {
                    return true;
                }
                key_contains(ks.sk, lhs) && !self.negate
            }
            LogicalKind::In { lhs, .. }
            | LogicalKind::Contains { lhs, .. }
            | LogicalKind::AttributeType { lhs, .. }
            | LogicalKind::AttributeExists { lhs } => {
                !key_contains(ks.pk, lhs) && !key_contains(ks.sk, lhs)
            }
        }
    }
}
