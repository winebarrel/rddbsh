// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! Per-command executors (port of src/execute).
pub mod alter_table_executor;
pub mod backup_restore_executor;
pub mod create_table_executor;
pub mod delete_executor;
pub mod describe_executor;
pub mod drop_table_executor;
pub mod insert_executor;
pub mod select_executor;
pub mod show_commands_executor;
pub mod show_tables_executor;
pub mod transact_executor;
pub mod update_executor;

/// Generates a fresh alias prefix per call.
pub type PrefixGen<'a> = dyn FnMut() -> String + 'a;

// DML helpers (dml agent).
pub mod consumed_capacity_totalizer;
pub mod rate_limiter;
pub mod select_cursor;
#[cfg(test)]
pub mod test_support;
pub mod transact_build;
pub mod write_common;

// DDL executor unit tests (ddl agent).
#[cfg(test)]
mod ddl_tests;

// Sort-key not-equal split unit tests.
#[cfg(test)]
mod not_equal_tests;
