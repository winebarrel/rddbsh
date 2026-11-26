// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! rddbsh — a Rust port of awslabs/dynamodb-shell (ddbsh).
//!
//! The engine (lexer, parser, expression engine, planner, executors, renderer)
//! is SDK-free and terminal-free; DynamoDB is reached through the
//! [`client::DdbClient`] seam, implemented by [`wire::WireClient`] over a
//! SigV4-signed HTTPS transport.

pub mod cli;
pub mod client;
pub mod completion;
pub mod error;
pub mod execute;
pub mod expressions;
pub mod outcome;
pub mod parser;
pub mod planner;
pub mod render;
pub mod retry;
pub mod session;
pub mod statement;
pub mod table_schema;
pub mod value;
pub mod wire;

/// The ddbsh version string.
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}
