// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! Helpers shared by the write executors (port of execute/write_common.hpp).

use crate::client::*;
use crate::error::Result;
use crate::expressions::{KeyMap, NeLeg, QueryKeyPlan, SymbolTable, Where};
use crate::outcome::{
    BulkVerb, BulkWriteOutcome, ExecOutcome, ExplainBlock, ExplainFormat, ExplainOutcome,
    OutcomeSink,
};
use crate::value::ValueMap;

pub use crate::planner::select_planner::key_of;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteReadApi {
    Query,
    Scan,
}

#[derive(Debug, Clone)]
pub struct WriteReadPlan {
    pub api: WriteReadApi,
    pub query: QueryIn,
    /// The upper leg of a not-equal split: enumerated after `query` (the
    /// lower leg) is exhausted. Unset otherwise.
    pub query_upper: Option<QueryIn>,
    pub scan: ScanIn,
}

impl WriteReadPlan {
    /// Move on to the upper leg of a not-equal split once the current read
    /// is exhausted. False when there is no further leg.
    pub fn next_leg(&mut self) -> bool {
        if self.api != WriteReadApi::Query {
            return false;
        }
        match self.query_upper.take() {
            Some(upper) => {
                self.query = upper;
                true
            }
            None => false,
        }
    }
}

/// The base-table key of a read row (attributes sorted by name, first wins).
pub fn row_key(item: &ItemRow) -> ValueMap {
    let mut sorted = KeyMap::new();
    for (name, value) in &item.attributes {
        sorted.entry(name.clone()).or_insert_with(|| value.clone());
    }
    key_of(&sorted)
}

pub fn project_base_keys(base_pk: &str, base_rk: &str, st: &mut SymbolTable) -> String {
    let mut projection = st.new_attribute(base_pk);
    if !base_rk.is_empty() {
        projection.push_str(", ");
        projection.push_str(&st.new_attribute(base_rk));
    }
    projection
}

#[allow(clippy::too_many_arguments)]
pub fn build_write_read(
    table: &str,
    index: &Option<String>,
    where_: Option<&Where>,
    qplan: &Option<QueryKeyPlan>,
    base_pk: &str,
    base_rk: &str,
    ratelimit: bool,
    st: &mut SymbolTable,
) -> WriteReadPlan {
    let rcc = if ratelimit {
        ReturnConsumedCapacity::Total
    } else {
        ReturnConsumedCapacity::None
    };
    if let (Some(qp), Some(w)) = (qplan, where_) {
        // A not-equal split renders its upper leg from a copy of the table as
        // it is now, so both legs carry the same tokens (as the SELECT planner).
        let fresh = st.clone();
        let build = |leg_st: &mut SymbolTable, leg: NeLeg| {
            let mut r = QueryIn {
                table_name: table.to_string(),
                index_name: index.clone(),
                consistent_read: false,
                return_consumed_capacity: rcc,
                ..Default::default()
            };
            r.projection_expression = Some(project_base_keys(base_pk, base_rk, leg_st));
            r.key_condition_expression = w.query_key_condition_expression(qp, Some(leg_st), leg);
            let filter = w.query_filter_expression(qp, Some(leg_st));
            if !filter.is_empty() {
                r.filter_expression = Some(filter);
            }
            r.expression_attribute_names = leg_st.names_vec();
            r.expression_attribute_values = leg_st.values_vec();
            r
        };
        let query = build(st, NeLeg::Lower);
        let query_upper = qp.ne_split.as_ref().map(|_| {
            let mut upper_st = fresh;
            build(&mut upper_st, NeLeg::Upper)
        });
        return WriteReadPlan {
            api: WriteReadApi::Query,
            query,
            query_upper,
            scan: ScanIn::default(),
        };
    }
    let mut r = ScanIn {
        table_name: table.to_string(),
        index_name: index.clone(),
        consistent_read: false,
        return_consumed_capacity: rcc,
        ..Default::default()
    };
    r.projection_expression = Some(project_base_keys(base_pk, base_rk, st));
    if let Some(w) = where_ {
        let filter = w.scan_filter_expression(Some(st));
        if !filter.is_empty() {
            r.filter_expression = Some(filter);
        }
    }
    r.expression_attribute_names = st.names_vec();
    r.expression_attribute_values = st.values_vec();
    WriteReadPlan {
        api: WriteReadApi::Scan,
        query: QueryIn::default(),
        query_upper: None,
        scan: r,
    }
}

/// One page of the bulk read.
pub struct ReadPage {
    pub items: Vec<ItemRow>,
    pub last_key: Option<ValueMap>,
    pub cc: Option<ConsumedCapacity>,
}

/// Issue the read for one page starting at `start_key`.
pub fn read_page(
    client: &mut dyn DdbClient,
    read: &WriteReadPlan,
    start_key: &Option<ValueMap>,
) -> Result<ReadPage> {
    match read.api {
        WriteReadApi::Query => {
            let mut req = read.query.clone();
            req.exclusive_start_key = start_key.clone();
            let r = client.query(&req)?;
            Ok(ReadPage {
                items: r.items,
                last_key: r.last_evaluated_key,
                cc: r.consumed_capacity,
            })
        }
        WriteReadApi::Scan => {
            let mut req = read.scan.clone();
            req.exclusive_start_key = start_key.clone();
            let r = client.scan(&req)?;
            Ok(ReadPage {
                items: r.items,
                last_key: r.last_evaluated_key,
                cc: r.consumed_capacity,
            })
        }
    }
}

/// The page's read request as an EXPLAIN request + API name.
pub fn read_explain_request(
    read: &WriteReadPlan,
    start_key: &Option<ValueMap>,
) -> (&'static str, ExplainRequest) {
    match read.api {
        WriteReadApi::Query => {
            let mut req = read.query.clone();
            req.exclusive_start_key = start_key.clone();
            ("Query", ExplainRequest::Query(req))
        }
        WriteReadApi::Scan => {
            let mut req = read.scan.clone();
            req.exclusive_start_key = start_key.clone();
            ("Scan", ExplainRequest::Scan(req))
        }
    }
}

pub fn explain_block(
    client: &mut dyn DdbClient,
    api_name: &str,
    request: &ExplainRequest,
) -> Result<ExplainBlock> {
    let body = client.explain_request(request)?;
    Ok(ExplainBlock {
        api_name: api_name.to_string(),
        body,
        format: ExplainFormat::Pretty,
    })
}

/// Emit one EXPLAIN block as its own fragment (the streaming bulk writes).
pub fn emit_explain_block(sink: &mut dyn OutcomeSink, block: ExplainBlock) {
    sink.emit(ExecOutcome::Explain(ExplainOutcome {
        blocks: vec![block],
        trailing_status: None,
    }));
}

/// Emit one RETURN VALUES item of a bulk write as a partial BulkWriteOutcome
/// (renders the item line only; the accounting line follows at the end).
pub fn emit_returned_item(sink: &mut dyn OutcomeSink, verb: BulkVerb, item: ItemRow) {
    sink.emit(ExecOutcome::BulkWrite(BulkWriteOutcome {
        verb,
        returned_items: vec![item],
        partial: true,
        ..Default::default()
    }));
}
