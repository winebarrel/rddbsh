// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! Resolve a table's (or index's) key schema via DescribeTable
//! (port of planner/get_key_schema.cpp).

use crate::client::{DdbClient, DescribeTableIn, TableKeySchema};
use crate::error::{fail, Errc, Result};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResolvedKeySchema {
    pub pk: Vec<String>,
    pub rk: Vec<String>,
}

fn from_table_key(ks: &TableKeySchema) -> ResolvedKeySchema {
    ResolvedKeySchema {
        pk: ks.hash_keys.clone(),
        rk: ks.range_keys.clone(),
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
        return Ok(from_table_key(&desc.key_schema));
    }
    for lsi in &desc.local_secondary_indexes {
        if lsi.index_name == index {
            return Ok(from_table_key(&lsi.key_schema));
        }
    }
    for gsi in &desc.global_secondary_indexes {
        if gsi.index_name == index {
            return Ok(from_table_key(&gsi.key_schema));
        }
    }
    fail(
        Errc::NoSuchTable,
        format!("Unable to find primary key for {}.{}", table, index),
    )
}
