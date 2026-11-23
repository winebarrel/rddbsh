// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! The engine's DynamoDB attribute value model (port of include/ddbsh/value.hpp).

/// An ordered attribute map (insertion order preserved, like the C++ ValueMap).
pub type ValueMap = Vec<(String, Value)>;
pub type ValueList = Vec<Value>;

#[derive(Debug, Clone, PartialEq, Default)]
pub enum Value {
    /// String.
    S(String),
    /// Number (exact decimal string).
    N(String),
    /// Binary (base64 text).
    B(String),
    Bool(bool),
    #[default]
    Null,
    L(ValueList),
    M(ValueMap),
    SS(Vec<String>),
    NS(Vec<String>),
    BS(Vec<String>),
    /// An unbound positional parameter marker `?` (1-based ordinal).
    Param(u32),
}

impl Value {
    pub fn string(s: impl Into<String>) -> Value {
        Value::S(s.into())
    }
    pub fn number(s: impl Into<String>) -> Value {
        Value::N(s.into())
    }
    pub fn is_param(&self) -> bool {
        matches!(self, Value::Param(_))
    }
    /// Collect mutable references to every parameter marker inside this value.
    pub fn collect_param_slots<'a>(&'a mut self, out: &mut Vec<&'a mut Value>) {
        match self {
            Value::Param(_) => out.push(self),
            Value::L(list) => list.iter_mut().for_each(|v| v.collect_param_slots(out)),
            Value::M(map) => map.iter_mut().for_each(|(_, v)| v.collect_param_slots(out)),
            _ => {}
        }
    }
    pub fn count_params(&self) -> usize {
        match self {
            Value::Param(_) => 1,
            Value::L(list) => list.iter().map(Value::count_params).sum(),
            Value::M(map) => map.iter().map(|(_, v)| v.count_params()).sum(),
            _ => 0,
        }
    }
}
