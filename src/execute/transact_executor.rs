// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! BEGIN ... COMMIT/ROLLBACK (port of execute/transact_executor.cpp).

use super::transact_build::*;
use super::write_common::explain_block;
use super::PrefixGen;
use crate::client::*;
use crate::error::{Errc, Error, Result};
use crate::outcome::{
    ExecOutcome, ExplainOutcome, OutcomeSink, StatusOutcome, TransactOutcome, TransactReadOutcome,
};
use crate::parser::ast::{CommandNode, TransactAccess, TransactCmd};

/// Absorb a not-reducible error into the ABORT diagnostics; propagate anything else.
fn absorb_or_propagate(
    error: Error,
    aborted: &mut bool,
    diagnostics: &mut Vec<String>,
) -> Result<()> {
    if error.code == Errc::NotReducibleToGetitem {
        if !error.message.is_empty() {
            diagnostics.push(error.message);
        }
        *aborted = true;
        return Ok(());
    }
    Err(error)
}

fn abort(diagnostics: Vec<String>) -> ExecOutcome {
    ExecOutcome::Transact(TransactOutcome {
        token: "ABORT".into(),
        diagnostics,
    })
}

fn explain_single(
    client: &mut dyn DdbClient,
    api: &str,
    request: ExplainRequest,
) -> Result<ExecOutcome> {
    let block = explain_block(client, api, &request)?;
    Ok(ExecOutcome::Explain(ExplainOutcome {
        blocks: vec![block],
        trailing_status: None,
    }))
}

fn execute_transact_read(
    cmd: &TransactCmd,
    client: &mut dyn DdbClient,
    prefix_gen: &mut PrefixGen,
    explain: bool,
    sink: &mut dyn OutcomeSink,
) -> Result<()> {
    let mut aborted = cmd.rollback;
    let mut diagnostics = Vec::new();
    let mut gets = Vec::new();
    for item in &cmd.items {
        let CommandNode::Select(select) = item else {
            aborted = true;
            continue;
        };
        match build_transact_get(select, client, prefix_gen) {
            Ok(get) => gets.push(get),
            Err(e) => absorb_or_propagate(e, &mut aborted, &mut diagnostics)?,
        }
    }
    if aborted {
        sink.emit(abort(diagnostics));
        return Ok(());
    }
    let request = TransactGetItemsIn { items: gets };
    if explain {
        sink.emit(explain_single(
            client,
            "TransactGetItems",
            ExplainRequest::TransactGetItems(request),
        )?);
        return Ok(());
    }
    match client.transact_get_items(&request) {
        Ok(out) => {
            let items = out
                .responses
                .into_iter()
                .map(|r| r.unwrap_or_default())
                .collect();
            sink.emit(ExecOutcome::TransactRead(TransactReadOutcome { items }));
            Ok(())
        }
        Err(e) => {
            sink.emit(abort(vec![]));
            Err(e)
        }
    }
}

fn execute_transact_write(
    cmd: &TransactCmd,
    client: &mut dyn DdbClient,
    prefix_gen: &mut PrefixGen,
    explain: bool,
    sink: &mut dyn OutcomeSink,
) -> Result<()> {
    let mut aborted = cmd.rollback;
    let mut diagnostics = Vec::new();
    let mut writes: Vec<TransactWriteItem> = Vec::new();
    for item in &cmd.items {
        let step: Result<Vec<TransactWriteItem>> = match item {
            CommandNode::Insert(node) => build_transact_puts(node, client, prefix_gen)
                .map(|puts| puts.into_iter().map(TransactWriteItem::Put).collect()),
            CommandNode::Update(node) => build_transact_update(node, client, prefix_gen)
                .map(|u| vec![TransactWriteItem::Update(u)]),
            CommandNode::Delete(node) => build_transact_delete(node, client, prefix_gen)
                .map(|d| vec![TransactWriteItem::Delete(d)]),
            CommandNode::Select(node) => build_transact_condition_check(node, client, prefix_gen)
                .map(|c| vec![TransactWriteItem::ConditionCheck(c)]),
            _ => {
                aborted = true;
                Ok(vec![])
            }
        };
        match step {
            Ok(items) => writes.extend(items),
            Err(e) => absorb_or_propagate(e, &mut aborted, &mut diagnostics)?,
        }
    }
    if aborted {
        sink.emit(abort(diagnostics));
        return Ok(());
    }
    let request = TransactWriteItemsIn { items: writes };
    if explain {
        sink.emit(explain_single(
            client,
            "TransactWriteItems",
            ExplainRequest::TransactWriteItems(request),
        )?);
        return Ok(());
    }
    match client.transact_write_items(&request) {
        Ok(()) => {
            sink.emit(ExecOutcome::Status(StatusOutcome {
                token: "COMMIT".into(),
                diagnostics: vec![],
            }));
            Ok(())
        }
        Err(e) => {
            sink.emit(abort(vec![]));
            Err(e)
        }
    }
}

pub fn execute_transact(
    cmd: &TransactCmd,
    client: &mut dyn DdbClient,
    prefix_gen: &mut PrefixGen,
    explain: bool,
    sink: &mut dyn OutcomeSink,
) -> Result<()> {
    if cmd.items.is_empty() {
        sink.emit(ExecOutcome::Empty);
        return Ok(());
    }
    match cmd.access {
        TransactAccess::Read => execute_transact_read(cmd, client, prefix_gen, explain, sink),
        TransactAccess::Write => execute_transact_write(cmd, client, prefix_gen, explain, sink),
    }
}
