// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! Builders reducing inner transaction statements to Transact* items
//! (port of execute/transact_build.cpp).

use super::delete_executor::make_delete_request;
use super::insert_executor::{make_put_request, validate_insert_rows};
use super::update_executor::make_update_request;
use super::write_common::key_of;
use super::PrefixGen;
use crate::client::*;
use crate::error::{fail, Errc, Result};
use crate::expressions::{QueryKeyPlan, SymbolTable};
use crate::parser::ast::{DeleteCmd, ExistsMode, InsertCmd, SelectCmd, UpdateCmd};
use crate::planner::get_key_schema;
use crate::planner::select_planner::serialize_projection;

pub const SELECT_NOT_REDUCIBLE_MESSAGE: &str =
    "The SELECT cannot be used in a transaction (it cannot be reduced to a GetItem() request).";
pub const SELECT_NO_EXISTS_MESSAGE: &str =
    "A SELECT command without an EXISTS or NOT EXISTS cannot be used in a write transaction.";

fn first(v: &[String]) -> String {
    v.first().cloned().unwrap_or_default()
}

pub fn build_transact_get(
    cmd: &SelectCmd,
    client: &mut dyn DdbClient,
    prefix_gen: &mut PrefixGen,
) -> Result<TransactGet> {
    let schema = get_key_schema(client, &cmd.table, "")?;
    let where_ = match &cmd.where_ {
        Some(w) if cmd.index.is_none() && w.can_getitem(&schema.pk, &schema.rk) => w,
        _ => return fail(Errc::NotReducibleToGetitem, SELECT_NOT_REDUCIBLE_MESSAGE),
    };
    let mut st = SymbolTable::new(prefix_gen());
    let mut get = TransactGet {
        table_name: cmd.table.clone(),
        ..Default::default()
    };
    if !cmd.projection.is_empty() {
        get.projection_expression = Some(serialize_projection(&cmd.projection, &mut st));
    }
    get.expression_attribute_names = st.names_vec();
    get.key = key_of(&where_.get_gud_key(&schema.pk, &schema.rk));
    Ok(get)
}

pub fn build_transact_puts(
    cmd: &InsertCmd,
    client: &mut dyn DdbClient,
    prefix_gen: &mut PrefixGen,
) -> Result<Vec<TransactPut>> {
    let schema = get_key_schema(client, &cmd.table_name, "")?;
    let pk = first(&schema.pk);
    validate_insert_rows(cmd)?;
    Ok((0..cmd.rows.len())
        .map(|row| {
            let req = make_put_request(cmd, row, &pk, prefix_gen);
            TransactPut {
                table_name: req.table_name,
                item: req.item,
                condition_expression: req.condition_expression,
                expression_attribute_names: req.expression_attribute_names,
                expression_attribute_values: req.expression_attribute_values,
            }
        })
        .collect())
}

pub fn build_transact_update(
    cmd: &UpdateCmd,
    client: &mut dyn DdbClient,
    prefix_gen: &mut PrefixGen,
) -> Result<TransactUpdate> {
    let schema = get_key_schema(client, &cmd.table, "")?;
    let (pk, rk) = (first(&schema.pk), first(&schema.rk));
    let where_ = match &cmd.where_ {
        Some(w) if w.can_update_item(&schema.pk, &schema.rk) => w,
        _ => return fail(Errc::NotReducibleToGetitem, ""),
    };
    let mut st = SymbolTable::new(prefix_gen());
    let req = make_update_request(cmd, &pk, &rk, &mut st);
    Ok(TransactUpdate {
        table_name: req.table_name,
        key: key_of(&where_.get_gud_key(&schema.pk, &schema.rk)),
        update_expression: req.update_expression,
        condition_expression: req.condition_expression,
        expression_attribute_names: req.expression_attribute_names,
        expression_attribute_values: req.expression_attribute_values,
    })
}

pub fn build_transact_delete(
    cmd: &DeleteCmd,
    client: &mut dyn DdbClient,
    prefix_gen: &mut PrefixGen,
) -> Result<TransactDelete> {
    let schema = get_key_schema(client, &cmd.table, "")?;
    let (pk, rk) = (first(&schema.pk), first(&schema.rk));
    let where_ = match &cmd.where_ {
        Some(w) if w.can_delete_item(&schema.pk, &schema.rk) => w,
        _ => return fail(Errc::NotReducibleToGetitem, ""),
    };
    let mut st = SymbolTable::new(prefix_gen());
    let req = make_delete_request(cmd, &pk, &rk, &mut st);
    Ok(TransactDelete {
        table_name: req.table_name,
        key: key_of(&where_.get_gud_key(&schema.pk, &schema.rk)),
        condition_expression: req.condition_expression,
        expression_attribute_names: req.expression_attribute_names,
        expression_attribute_values: req.expression_attribute_values,
    })
}

pub fn build_transact_condition_check(
    cmd: &SelectCmd,
    client: &mut dyn DdbClient,
    prefix_gen: &mut PrefixGen,
) -> Result<TransactConditionCheck> {
    let check_func = match cmd.exists_mode {
        ExistsMode::Exists => "attribute_exists",
        ExistsMode::NotExists => "attribute_not_exists",
        ExistsMode::None => return fail(Errc::NotReducibleToGetitem, SELECT_NO_EXISTS_MESSAGE),
    };
    let schema = get_key_schema(client, &cmd.table, "")?;
    let pk = first(&schema.pk);
    let Some(where_) = &cmd.where_ else {
        return fail(Errc::NotReducibleToGetitem, "");
    };
    let mut cc = TransactConditionCheck {
        table_name: cmd.table.clone(),
        key: key_of(&where_.get_gud_key(&schema.pk, &schema.rk)),
        ..Default::default()
    };
    let mut st = SymbolTable::new(prefix_gen());
    let plan = QueryKeyPlan {
        pk: schema.pk.clone(),
        sk_prefix: schema.rk.clone(),
    };
    let mut filter = where_.query_filter_expression(&plan, Some(&mut st));
    if !filter.is_empty() {
        filter.push_str(" AND ");
    }
    if cmd.projection.is_empty() {
        filter.push_str(&format!("{}({})", check_func, st.new_attribute(&pk)));
    } else {
        for p in &cmd.projection {
            filter.push_str(&format!("{}({})", check_func, st.new_attribute(p)));
        }
    }
    cc.condition_expression = filter;
    cc.expression_attribute_names = st.names_vec();
    cc.expression_attribute_values = st.values_vec();
    Ok(cc)
}
