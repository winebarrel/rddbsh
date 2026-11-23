// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! Deterministic `#a<prefix><n>` / `:v<prefix><n>` alias allocation
//! (port of expressions/symbol_table.{hpp,cpp}).

use std::collections::BTreeMap;

use crate::value::Value;

#[derive(Debug, Clone)]
pub struct SymbolTable {
    prefix: String,
    attr_ix: u32,
    value_ix: u32,
    /// real name -> token
    names: BTreeMap<String, String>,
    /// token -> value
    values: BTreeMap<String, Value>,
}

impl SymbolTable {
    pub fn new(prefix: impl Into<String>) -> SymbolTable {
        SymbolTable {
            prefix: prefix.into(),
            attr_ix: 1,
            value_ix: 1,
            names: BTreeMap::new(),
            values: BTreeMap::new(),
        }
    }

    /// Alias a (possibly dotted / subscripted) attribute path segment by segment.
    pub fn new_attribute(&mut self, name: &str) -> String {
        name.split('.')
            .map(|segment| self.new_segment(segment))
            .collect::<Vec<_>>()
            .join(".")
    }

    fn new_segment(&mut self, name: &str) -> String {
        match name.find('[') {
            Some(bracket) => {
                let mut token = self.new_bare(&name[..bracket]);
                token.push_str(&name[bracket..]);
                token
            }
            None => self.new_bare(name),
        }
    }

    fn new_bare(&mut self, name: &str) -> String {
        if let Some(existing) = self.names.get(name) {
            return existing.clone();
        }
        let token = format!("#a{}{}", self.prefix, self.attr_ix);
        self.attr_ix += 1;
        self.names.insert(name.to_string(), token.clone());
        token
    }

    pub fn new_value(&mut self, value: Value) -> String {
        let token = format!(":v{}{}", self.prefix, self.value_ix);
        self.value_ix += 1;
        self.values.insert(token.clone(), value);
        token
    }

    pub fn has_names(&self) -> bool {
        !self.names.is_empty()
    }
    pub fn has_values(&self) -> bool {
        !self.values.is_empty()
    }

    /// token -> real name, sorted by token.
    pub fn get_names(&self) -> BTreeMap<String, String> {
        self.names
            .iter()
            .map(|(name, token)| (token.clone(), name.clone()))
            .collect()
    }

    pub fn get_values(&self) -> &BTreeMap<String, Value> {
        &self.values
    }

    /// Names as the client-seam pair list (sorted by token).
    pub fn names_vec(&self) -> Vec<(String, String)> {
        self.get_names().into_iter().collect()
    }

    /// Values as the client-seam ValueMap (sorted by token).
    pub fn values_vec(&self) -> Vec<(String, Value)> {
        self.values
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}
