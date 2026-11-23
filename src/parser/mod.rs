// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! Lexer + recursive-descent parser (port of src/parser: lexer.l, grammar.yy, driver.cpp).
pub mod ast;
pub mod grammar;
pub mod help_text;
pub mod lexer;
pub mod param_bind;

pub use grammar::{parse_batch, parse_one};
pub use param_bind::count_param_markers;
