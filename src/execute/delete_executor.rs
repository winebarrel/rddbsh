// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! DELETE (port of execute/delete_executor.cpp).

use super::rate_limiter::RateLimiter;
use super::write_common::*;
use super::PrefixGen;
use crate::client::*;
use crate::error::{Errc, Result};
use crate::expressions::{QueryKeyPlan, SymbolTable};
use crate::outcome::{BulkVerb, BulkWriteOutcome, ExecOutcome, ExplainOutcome, OutcomeSink};
use crate::parser::ast::DeleteCmd;
use crate::planner::get_key_schema;
use crate::value::ValueMap;

pub fn make_delete_request(
    cmd: &DeleteCmd,
    pk: &str,
    rk: &str,
    st: &mut SymbolTable,
) -> DeleteItemIn {
    let mut request = DeleteItemIn {
        table_name: cmd.table.clone(),
        return_values: cmd.return_value,
        ..Default::default()
    };
    if let Some(w) = &cmd.where_ {
        let cond = w.update_delete_condition_check(pk, rk, Some(st));
        if !cond.is_empty() {
            request.condition_expression = Some(cond);
        }
    }
    request.expression_attribute_names = st.names_vec();
    request.expression_attribute_values = st.values_vec();
    if cmd.ratelimit.is_some() {
        request.return_consumed_capacity = ReturnConsumedCapacity::Total;
    }
    request
}

pub fn execute_delete(
    cmd: &DeleteCmd,
    client: &mut dyn DdbClient,
    prefix_gen: &mut PrefixGen,
    explain: bool,
    sink: &mut dyn OutcomeSink,
) -> Result<()> {
    let base = get_key_schema(client, &cmd.table, "")?;
    let base_pk = base.pk.first().cloned().unwrap_or_default();
    let base_rk = base.rk.first().cloned().unwrap_or_default();
    let where_ = cmd.where_.as_ref();
    let mut write_st = SymbolTable::new(prefix_gen());
    let write_template = make_delete_request(cmd, &base_pk, &base_rk, &mut write_st);
    let want_rv = cmd.return_value != ReturnValue::None;
    let mut limiter = if explain {
        None
    } else {
        cmd.ratelimit.map(|rl| RateLimiter::new(rl.rcu, rl.wcu))
    };

    if let Some(w) = where_.filter(|w| w.can_delete_item(&base.pk, &base.rk)) {
        let mut request = write_template.clone();
        request.key = key_of(&w.get_gud_key(&base.pk, &base.rk));
        if explain {
            let block = explain_block(client, "DeleteItem", &ExplainRequest::DeleteItem(request))?;
            sink.emit(ExecOutcome::Explain(ExplainOutcome {
                blocks: vec![block],
                trailing_status: None,
            }));
            return Ok(());
        }
        if let Some(l) = &mut limiter {
            l.writelimit();
        }
        let mut acct = BulkWriteOutcome {
            verb: BulkVerb::Del,
            ..Default::default()
        };
        match client.delete_item(&request) {
            Ok(result) => {
                acct.modified = 1;
                if want_rv {
                    acct.returned_items
                        .push(result.attributes.clone().unwrap_or_default());
                }
                if let Some(l) = &mut limiter {
                    l.consume_writes(&result.consumed_capacity);
                }
            }
            Err(e) if e.code == Errc::ConditionalCheckFailed => acct.ccf = 1,
            Err(e) => return Err(e),
        }
        sink.emit(ExecOutcome::BulkWrite(acct));
        return Ok(());
    }

    let qplan: Option<QueryKeyPlan> = match &cmd.index {
        None => where_.and_then(|w| w.plan_query(&base.pk, &base.rk, Some(&base.key_types))),
        Some(index) => {
            let idx = get_key_schema(client, &cmd.table, index)?;
            where_.and_then(|w| w.plan_query(&idx.pk, &idx.rk, Some(&idx.key_types)))
        }
    };
    let mut read_st = SymbolTable::new(prefix_gen());
    let mut read = build_write_read(
        &cmd.table,
        &cmd.index,
        where_,
        &qplan,
        &base_pk,
        &base_rk,
        cmd.ratelimit.is_some(),
        &mut read_st,
    );
    let mut acct = BulkWriteOutcome {
        verb: BulkVerb::Del,
        ..Default::default()
    };
    if explain {
        let (api, req) = read_explain_request(&read, &None);
        emit_explain_block(sink, explain_block(client, api, &req)?);
        if read.api == WriteReadApi::Query {
            // A not-equal split runs two Queries, so EXPLAIN prints both
            // Query blocks, lower leg first, and still no DeleteItem blocks.
            if let Some(upper) = &read.query_upper {
                let req = ExplainRequest::Query(upper.clone());
                emit_explain_block(sink, explain_block(client, "Query", &req)?);
            }
            return Ok(());
        }
    }
    let mut start_key: Option<ValueMap> = None;
    loop {
        let page = read_page(client, &read, &start_key)?;
        if let Some(l) = &mut limiter {
            l.consume_reads(&page.cc);
        }
        let mut page_writes = ExplainOutcome::default();
        for item in &page.items {
            let mut request = write_template.clone();
            request.key = row_key(item);
            if explain {
                page_writes.blocks.push(explain_block(
                    client,
                    "DeleteItem",
                    &ExplainRequest::DeleteItem(request),
                )?);
                continue;
            }
            acct.read += 1;
            if let Some(l) = &mut limiter {
                l.writelimit();
            }
            match client.delete_item(&request) {
                Ok(result) => {
                    acct.modified += 1;
                    if want_rv {
                        emit_returned_item(
                            sink,
                            BulkVerb::Del,
                            result.attributes.clone().unwrap_or_default(),
                        );
                    }
                    if let Some(l) = &mut limiter {
                        l.consume_writes(&result.consumed_capacity);
                    }
                }
                Err(e) if e.code == Errc::ConditionalCheckFailed => acct.ccf += 1,
                Err(e) => return Err(e),
            }
        }
        if !page_writes.blocks.is_empty() {
            sink.emit(ExecOutcome::Explain(page_writes));
        }
        match page.last_key {
            Some(k) if !k.is_empty() => start_key = Some(k),
            // The lower leg of a not-equal split is exhausted: enumerate the
            // upper leg from its first page, metered like a next page. The
            // counts and EXPLAIN blocks continue across legs.
            _ if read.next_leg() => start_key = None,
            _ => break,
        }
        if let Some(l) = &mut limiter {
            l.readlimit();
        }
    }
    if !explain {
        sink.emit(ExecOutcome::BulkWrite(acct));
    }
    Ok(())
}
