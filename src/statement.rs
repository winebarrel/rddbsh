// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! Statement kinds and the parsed-statement handle (port of include/ddbsh/statement.hpp).

pub use crate::parser::ast::ParsedStatement as Statement;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementKind {
    Empty,
    ShowVersion,
    ShowConnection,
    Quit,
    Connect,
    ShowTables,
    Describe,
    DropTable,
    CreateTable,
    AlterTable,
    Insert,
    SelectStatement,
    UpdateStatement,
    Del,
    Help,
    Transaction,
    CreateBackup,
    DropBackup,
    DescribeBackup,
    Restore,
    ShowBackups,
    ShowLimits,
    ShowCreateTable,
}
