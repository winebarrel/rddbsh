// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! UPDATE / UPSERT (port of execute/update_executor.cpp).

use super::rate_limiter::RateLimiter;
use super::write_common::*;
use super::PrefixGen;
use crate::client::*;
use crate::error::{Errc, Result};
use crate::expressions::{QueryKeyPlan, SymbolTable};
use crate::outcome::{BulkVerb, BulkWriteOutcome, ExecOutcome, ExplainOutcome, OutcomeSink};
use crate::parser::ast::{Mutation, UpdateCmd, UpdateSetElement};
use crate::planner::get_key_schema;
use crate::value::ValueMap;

fn serialize_set_element(elem: &UpdateSetElement, st: &mut SymbolTable) -> String {
    let mut rv = format!("{} = ", st.new_attribute(&elem.lhs));
    if !elem.rhs_attribute.is_empty() {
        rv.push_str(&st.new_attribute(&elem.rhs_attribute));
    } else if !elem.if_not_exists_lhs.is_empty() {
        rv.push_str("if_not_exists(");
        rv.push_str(&st.new_attribute(&elem.if_not_exists_lhs));
        rv.push(',');
        rv.push_str(&st.new_value(elem.if_not_exists_value.clone().unwrap_or_default()));
        rv.push(')');
    }
    rv.push_str(&elem.op);
    if let Some(v) = &elem.rhs_value {
        rv.push_str(&st.new_value(v.clone()));
    }
    rv
}

pub fn make_update_request(
    cmd: &UpdateCmd,
    pk: &str,
    rk: &str,
    st: &mut SymbolTable,
) -> UpdateItemIn {
    let mut request = UpdateItemIn {
        table_name: cmd.table.clone(),
        return_values: cmd.return_value,
        ..Default::default()
    };
    request.update_expression = if cmd.mutation == Mutation::Set {
        let parts: Vec<String> = cmd
            .set
            .iter()
            .map(|e| serialize_set_element(e, st))
            .collect();
        format!("SET {}", parts.join(", "))
    } else {
        let parts: Vec<String> = cmd.remove.iter().map(|r| st.new_attribute(r)).collect();
        format!("REMOVE {}", parts.join(", "))
    };
    let mut cond = String::new();
    if !cmd.upsert {
        cond = format!("attribute_exists({})", st.new_attribute(pk));
    }
    if let Some(w) = &cmd.where_ {
        let qf = w.update_delete_condition_check(pk, rk, Some(st));
        if !qf.is_empty() {
            cond = if cond.is_empty() {
                qf
            } else {
                format!("{} AND {}", cond, qf)
            };
        }
    }
    if !cond.is_empty() {
        request.condition_expression = Some(cond);
    }
    request.expression_attribute_names = st.names_vec();
    request.expression_attribute_values = st.values_vec();
    if cmd.ratelimit.is_some() {
        request.return_consumed_capacity = ReturnConsumedCapacity::Total;
    }
    request
}

pub fn execute_update(
    cmd: &UpdateCmd,
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
    let write_template = make_update_request(cmd, &base_pk, &base_rk, &mut write_st);
    let verb = if cmd.upsert {
        BulkVerb::Upsert
    } else {
        BulkVerb::Update
    };
    let want_rv = cmd.return_value != ReturnValue::None;
    let mut limiter = if explain {
        None
    } else {
        cmd.ratelimit.map(|rl| RateLimiter::new(rl.rcu, rl.wcu))
    };

    if let Some(w) = where_.filter(|w| w.can_update_item(&base.pk, &base.rk)) {
        let mut request = write_template.clone();
        request.key = key_of(&w.get_gud_key(&base.pk, &base.rk));
        if explain {
            let block = explain_block(client, "UpdateItem", &ExplainRequest::UpdateItem(request))?;
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
            verb,
            ..Default::default()
        };
        match client.update_item(&request) {
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
        None => where_.and_then(|w| w.plan_query(&base.pk, &base.rk)),
        Some(index) => {
            let idx = get_key_schema(client, &cmd.table, index)?;
            where_.and_then(|w| w.plan_query(&idx.pk, &idx.rk))
        }
    };
    let mut read_st = SymbolTable::new(prefix_gen());
    let read = build_write_read(
        &cmd.table,
        &cmd.index,
        where_,
        &qplan,
        &base_pk,
        &base_rk,
        cmd.ratelimit.is_some(),
        &mut read_st,
    );
    let mut explain_out = ExplainOutcome::default();
    let mut acct = BulkWriteOutcome {
        verb,
        ..Default::default()
    };
    let mut start_key: Option<ValueMap> = None;
    loop {
        if explain {
            let (api, req) = read_explain_request(&read, &start_key);
            explain_out.blocks.push(explain_block(client, api, &req)?);
        }
        let page = read_page(client, &read, &start_key)?;
        if let Some(l) = &mut limiter {
            l.consume_reads(&page.cc);
        }
        for item in &page.items {
            let mut request = write_template.clone();
            request.key = row_key(item);
            if explain {
                explain_out.blocks.push(explain_block(
                    client,
                    "UpdateItem",
                    &ExplainRequest::UpdateItem(request),
                )?);
                continue;
            }
            acct.read += 1;
            if let Some(l) = &mut limiter {
                l.writelimit();
            }
            match client.update_item(&request) {
                Ok(result) => {
                    acct.modified += 1;
                    if want_rv {
                        acct.returned_items
                            .push(result.attributes.clone().unwrap_or_default());
                    }
                    if let Some(l) = &mut limiter {
                        l.consume_writes(&result.consumed_capacity);
                    }
                }
                Err(e) if e.code == Errc::ConditionalCheckFailed => acct.ccf += 1,
                Err(e) => return Err(e),
            }
        }
        match page.last_key {
            Some(k) if !k.is_empty() => {
                start_key = Some(k);
                if let Some(l) = &mut limiter {
                    l.readlimit();
                }
            }
            _ => break,
        }
    }
    if explain {
        sink.emit(ExecOutcome::Explain(explain_out));
    } else {
        sink.emit(ExecOutcome::BulkWrite(acct));
    }
    Ok(())
}
