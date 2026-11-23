// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! Typed execution outcomes (port of include/ddbsh/exec_outcome.hpp + outcome_sink.hpp).

use crate::client::{ConsumedCapacity, ItemRow, VectorConsumedCapacity};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct StatusOutcome {
    pub token: String,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ReportOutcome {
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SelectOutcome {
    pub items: Vec<ItemRow>,
    pub consumed_capacity: Option<ConsumedCapacity>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct VectorSearchRow {
    pub item: ItemRow,
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct VectorSearchOutcome {
    pub rows: Vec<VectorSearchRow>,
    pub vector_capacity: Option<VectorConsumedCapacity>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct WriteOutcome {
    pub tokens: Vec<String>,
    pub items: Vec<Option<ItemRow>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExplainFormat {
    #[default]
    Pretty,
    Compact,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExplainBlock {
    pub api_name: String,
    pub body: String,
    pub format: ExplainFormat,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ExplainOutcome {
    pub blocks: Vec<ExplainBlock>,
    pub trailing_status: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BulkVerb {
    #[default]
    Update,
    Upsert,
    Del,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct BulkWriteOutcome {
    pub verb: BulkVerb,
    pub read: i64,
    pub modified: i64,
    pub ccf: i64,
    pub consumed_capacity: Option<ConsumedCapacity>,
    pub returned_items: Vec<ItemRow>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TransactOutcome {
    pub token: String,
    pub diagnostics: Vec<String>,
}
impl Default for TransactOutcome {
    fn default() -> Self {
        TransactOutcome {
            token: "ABORT".into(),
            diagnostics: vec![],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TransactReadOutcome {
    pub items: Vec<ItemRow>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExecOutcome {
    Status(StatusOutcome),
    Report(ReportOutcome),
    Empty,
    Select(SelectOutcome),
    Write(WriteOutcome),
    Explain(ExplainOutcome),
    BulkWrite(BulkWriteOutcome),
    Transact(TransactOutcome),
    TransactRead(TransactReadOutcome),
    VectorSearch(VectorSearchOutcome),
    Multi(Vec<ExecOutcome>),
}

impl ExecOutcome {
    pub fn status(token: impl Into<String>) -> ExecOutcome {
        ExecOutcome::Status(StatusOutcome {
            token: token.into(),
            diagnostics: vec![],
        })
    }
}

/// Receives outcome fragments as they are produced (streaming execution).
pub trait OutcomeSink {
    fn emit(&mut self, fragment: ExecOutcome);
}

/// A sink that buffers every fragment.
#[derive(Default)]
pub struct BufferingSink {
    pub parts: Vec<ExecOutcome>,
}
impl OutcomeSink for BufferingSink {
    fn emit(&mut self, fragment: ExecOutcome) {
        self.parts.push(fragment);
    }
}

/// Adapts a closure into an OutcomeSink.
pub struct FnSink<F: FnMut(ExecOutcome)>(pub F);
impl<F: FnMut(ExecOutcome)> OutcomeSink for FnSink<F> {
    fn emit(&mut self, fragment: ExecOutcome) {
        (self.0)(fragment)
    }
}
