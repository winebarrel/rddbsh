//! INSERT / REPLACE (PutItem per row) (port of execute/insert_executor.cpp).

use super::rate_limiter::RateLimiter;
use super::write_common::explain_block;
use super::PrefixGen;
use crate::client::*;
use crate::error::{fail, Errc, Result};
use crate::expressions::SymbolTable;
use crate::outcome::{ExecOutcome, ExplainOutcome, OutcomeSink, WriteOutcome};
use crate::parser::ast::InsertCmd;
use crate::planner::get_key_schema;

pub fn validate_insert_rows(cmd: &InsertCmd) -> Result<()> {
    for (row, values) in cmd.rows.iter().enumerate() {
        if values.len() != cmd.columns.len() {
            return fail(
                Errc::InvalidArgument,
                format!(
                    "Unequal number of column names ({}) and values ({}) in tuple {}",
                    cmd.columns.len(),
                    values.len(),
                    row + 1
                ),
            );
        }
    }
    Ok(())
}

pub fn make_put_request(
    cmd: &InsertCmd,
    row: usize,
    pk: &str,
    prefix_gen: &mut PrefixGen,
) -> PutItemIn {
    let mut request = PutItemIn {
        table_name: cmd.table_name.clone(),
        item: cmd
            .columns
            .iter()
            .cloned()
            .zip(cmd.rows[row].iter().cloned())
            .collect(),
        return_values: cmd.return_value,
        ..Default::default()
    };
    if cmd.ratelimit.is_some() {
        request.return_consumed_capacity = ReturnConsumedCapacity::Total;
    }
    if cmd.insert {
        let mut st = SymbolTable::new(prefix_gen());
        let pk_token = st.new_attribute(pk);
        request.condition_expression = Some(format!("attribute_not_exists({})", pk_token));
        request.expression_attribute_names = st.names_vec();
    }
    request
}

fn base_pk(client: &mut dyn DdbClient, table: &str) -> Result<String> {
    let schema = get_key_schema(client, table, "")?;
    Ok(schema.pk.first().cloned().unwrap_or_default())
}

pub fn execute_insert_explain(
    cmd: &InsertCmd,
    client: &mut dyn DdbClient,
    prefix_gen: &mut PrefixGen,
) -> Result<ExecOutcome> {
    let pk = base_pk(client, &cmd.table_name)?;
    validate_insert_rows(cmd)?;
    let mut outcome = ExplainOutcome::default();
    for row in 0..cmd.rows.len() {
        let request = make_put_request(cmd, row, &pk, prefix_gen);
        outcome.blocks.push(explain_block(
            client,
            "PutItem",
            &ExplainRequest::PutItem(request),
        )?);
    }
    Ok(ExecOutcome::Explain(outcome))
}

pub fn execute_insert_streaming(
    cmd: &InsertCmd,
    client: &mut dyn DdbClient,
    prefix_gen: &mut PrefixGen,
    sink: &mut dyn OutcomeSink,
) -> Result<()> {
    let pk = base_pk(client, &cmd.table_name)?;
    validate_insert_rows(cmd)?;
    let token = if cmd.insert { "INSERT" } else { "REPLACE" };
    let want_rv = cmd.return_value != ReturnValue::None;
    let mut limiter = cmd.ratelimit.map(|rl| RateLimiter::new(rl.rcu, rl.wcu));
    for row in 0..cmd.rows.len() {
        let request = make_put_request(cmd, row, &pk, prefix_gen);
        let result = client.put_item(&request)?;
        let mut out = WriteOutcome {
            tokens: vec![token.to_string()],
            items: vec![],
        };
        if want_rv {
            out.items
                .push(Some(result.attributes.clone().unwrap_or_default()));
        }
        sink.emit(ExecOutcome::Write(out));
        if let Some(l) = &mut limiter {
            l.consume_writes(&result.consumed_capacity);
            l.writelimit();
        }
    }
    Ok(())
}
