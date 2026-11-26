// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! SELECT execution + EXPLAIN (port of execute/select_executor.cpp).

use super::select_cursor::open_select_cursor;
use crate::client::{DdbClient, ExplainRequest, ReturnConsumedCapacity};
use crate::error::Result;
use crate::outcome::*;
use crate::parser::ast::SelectCmd;
use crate::planner::{plan_select, SelectApi};

pub fn execute_select(
    cmd: &SelectCmd,
    client: &mut dyn DdbClient,
    prefix: &str,
) -> Result<ExecOutcome> {
    if cmd.search_vector.is_some() {
        let plan = plan_select(cmd, client, prefix)?;
        let result = client.search_vectors(&plan.search_vectors)?;
        let mut vout = VectorSearchOutcome::default();
        for row in result.results {
            vout.rows.push(VectorSearchRow {
                item: row.item,
                score: row.score,
            });
        }
        if cmd.rcc != ReturnConsumedCapacity::None {
            vout.vector_capacity = result.vector_capacity;
        }
        return Ok(ExecOutcome::VectorSearch(vout));
    }
    let mut cursor = open_select_cursor(cmd, client, prefix)?;
    let mut outcome = SelectOutcome::default();
    while let Some(page) = cursor.next_page(client)? {
        outcome.items.extend(page);
    }
    outcome.consumed_capacity = cursor.consumed_capacity();
    outcome.column_hint = cursor.column_hint().to_vec();
    Ok(ExecOutcome::Select(outcome))
}

pub fn execute_select_explain(
    cmd: &SelectCmd,
    client: &mut dyn DdbClient,
    prefix: &str,
) -> Result<ExecOutcome> {
    let plan = plan_select(cmd, client, prefix)?;
    let (api_name, request) = match plan.api {
        SelectApi::GetItem => ("GetItem", ExplainRequest::GetItem(plan.get_item)),
        SelectApi::Query => ("Query", ExplainRequest::Query(plan.query)),
        SelectApi::Scan => ("Scan", ExplainRequest::Scan(plan.scan)),
        SelectApi::SearchVectors => (
            "SearchVectors",
            ExplainRequest::SearchVectors(plan.search_vectors),
        ),
    };
    let body = client.explain_request(&request)?;
    Ok(ExecOutcome::Explain(ExplainOutcome {
        blocks: vec![ExplainBlock {
            api_name: api_name.to_string(),
            body,
            format: ExplainFormat::Pretty,
        }],
        trailing_status: None,
    }))
}

pub fn execute_select_streaming(
    cmd: &SelectCmd,
    client: &mut dyn DdbClient,
    prefix: &str,
    sink: &mut dyn OutcomeSink,
) -> Result<()> {
    if cmd.search_vector.is_some() {
        let buffered = execute_select(cmd, client, prefix)?;
        sink.emit(buffered);
        return Ok(());
    }
    let mut cursor = open_select_cursor(cmd, client, prefix)?;
    // Every fragment carries the column hint the output formats use.
    let mut emitted = false;
    while let Some(page) = cursor.next_page(client)? {
        sink.emit(ExecOutcome::Select(SelectOutcome {
            items: page,
            consumed_capacity: None,
            column_hint: cursor.column_hint().to_vec(),
        }));
        emitted = true;
    }
    // A SELECT that found nothing emits one empty fragment, so a formatted
    // output still opens (and closes) its result set. It renders nothing in
    // the default format.
    if !emitted {
        sink.emit(ExecOutcome::Select(SelectOutcome {
            column_hint: cursor.column_hint().to_vec(),
            ..Default::default()
        }));
    }
    if let Some(cc) = cursor.consumed_capacity() {
        sink.emit(ExecOutcome::Select(SelectOutcome {
            consumed_capacity: Some(cc),
            ..Default::default()
        }));
    }
    Ok(())
}
