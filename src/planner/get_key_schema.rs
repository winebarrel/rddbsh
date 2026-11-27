// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! Resolve a table's (or index's) key schema via DescribeTable
//! (port of planner/get_key_schema.cpp).

use crate::client::{DdbClient, DescribeTableIn, DescribeTableOut, TableKeySchema};
use crate::error::{fail, Errc, Result};
use crate::expressions::KeyTypes;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResolvedKeySchema {
    pub pk: Vec<String>,
    pub rk: Vec<String>,
    /// The BASE TABLE's key attributes, hash then range (equal to pk + rk when
    /// no index is named). The output formats show them first.
    pub table_keys: Vec<String>,
    /// Declared scalar types from the same response's AttributeDefinitions,
    /// by attribute name (empty when the client reports none). The planner's
    /// not-equal split checks a literal against them.
    pub key_types: KeyTypes,
}

fn from_table_key(ks: &TableKeySchema, desc: &DescribeTableOut) -> ResolvedKeySchema {
    let table = &desc.key_schema;
    ResolvedKeySchema {
        pk: ks.hash_keys.clone(),
        rk: ks.range_keys.clone(),
        table_keys: table
            .hash_keys
            .iter()
            .chain(&table.range_keys)
            .cloned()
            .collect(),
        key_types: desc
            .attribute_definitions
            .iter()
            .map(|d| (d.name.clone(), d.ty))
            .collect(),
    }
}

/// `index` empty => base table.
pub fn get_key_schema(
    client: &mut dyn DdbClient,
    table: &str,
    index: &str,
) -> Result<ResolvedKeySchema> {
    let desc = client.describe_table(&DescribeTableIn {
        table_name: table.to_string(),
    })?;
    if index.is_empty() {
        return Ok(from_table_key(&desc.key_schema, &desc));
    }
    for lsi in &desc.local_secondary_indexes {
        if lsi.index_name == index {
            return Ok(from_table_key(&lsi.key_schema, &desc));
        }
    }
    for gsi in &desc.global_secondary_indexes {
        if gsi.index_name == index {
            return Ok(from_table_key(&gsi.key_schema, &desc));
        }
    }
    fail(
        Errc::NoSuchTable,
        format!("Unable to find primary key for {}.{}", table, index),
    )
}
