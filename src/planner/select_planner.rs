//! The SELECT GetItem/Query/Scan/SearchVectors decision matrix
//! (port of planner/select_planner.cpp).

use super::get_key_schema::get_key_schema;
use crate::client::*;
use crate::error::{fail, Errc, Result};
use crate::expressions::{KeyMap, SymbolTable};
use crate::parser::ast::SelectCmd;
use crate::value::{Value, ValueMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectApi {
    GetItem,
    Query,
    #[default]
    Scan,
    SearchVectors,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SelectPlan {
    pub api: SelectApi,
    pub get_item: GetItemIn,
    pub query: QueryIn,
    pub scan: ScanIn,
    pub search_vectors: SearchVectorsIn,
}

pub fn serialize_projection(projection: &[String], st: &mut SymbolTable) -> String {
    projection
        .iter()
        .map(|a| st.new_attribute(a))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn key_of(key: &KeyMap) -> ValueMap {
    key.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

fn plan_search_vectors(
    cmd: &SelectCmd,
    vi: &VectorIndexSchema,
    alias_prefix: &str,
) -> Result<SelectPlan> {
    let Some(qv) = &cmd.search_vector else {
        return fail(
            Errc::InvalidArgument,
            format!(
                "A SELECT on vector index {} requires a NEAREST <n> TO [ ... ] clause.",
                vi.index_name
            ),
        );
    };
    if cmd.consistent {
        return fail(
            Errc::InvalidArgument,
            "CONSISTENT is not supported with vector search.",
        );
    }
    let topk = cmd.topk.unwrap_or(0);
    if topk <= 0 {
        return fail(
            Errc::InvalidArgument,
            "Vector search TopK must be a positive integer.",
        );
    }
    let Value::L(list) = qv else {
        return fail(
            Errc::InvalidArgument,
            "The NEAREST search vector must be a numeric list.",
        );
    };
    if list.iter().any(|e| !matches!(e, Value::N(_))) {
        return fail(
            Errc::InvalidArgument,
            "The NEAREST search vector must contain only numbers.",
        );
    }
    if vi.dimensions > 0 && list.len() as i64 != vi.dimensions {
        return fail(
            Errc::InvalidArgument,
            format!(
                "The search vector has {} values; vector index {} expects {}.",
                list.len(),
                vi.index_name,
                vi.dimensions
            ),
        );
    }
    let mut st = SymbolTable::new(alias_prefix);
    let mut plan = SelectPlan {
        api: SelectApi::SearchVectors,
        ..Default::default()
    };
    let r = &mut plan.search_vectors;
    r.table_name = cmd.table.clone();
    r.index_name = cmd.index.clone().unwrap_or_default();
    r.search_vector = list.clone();
    r.topk = topk;
    if !cmd.projection.is_empty() {
        r.projection_expression = Some(serialize_projection(&cmd.projection, &mut st));
    }
    if let Some(w) = &cmd.where_ {
        let filter = w.scan_filter_expression(Some(&mut st));
        if !filter.is_empty() {
            r.search_condition_expression = Some(filter);
        }
    }
    r.expression_attribute_names = st.names_vec();
    r.expression_attribute_values = st.values_vec();
    r.return_consumed_capacity = if cmd.ratelimit.is_some() {
        ReturnConsumedCapacity::Total
    } else {
        cmd.rcc
    };
    Ok(plan)
}

pub fn plan_select(
    cmd: &SelectCmd,
    client: &mut dyn DdbClient,
    alias_prefix: &str,
) -> Result<SelectPlan> {
    if let Some(index) = &cmd.index {
        let described = client.describe_table(&DescribeTableIn {
            table_name: cmd.table.clone(),
        })?;
        if let Some(vi) = described
            .vector_indexes
            .iter()
            .find(|vi| &vi.index_name == index)
        {
            return plan_search_vectors(cmd, vi, alias_prefix);
        }
    }
    if cmd.search_vector.is_some() {
        return fail(
            Errc::InvalidArgument,
            match &cmd.index {
                Some(i) => format!(
                    "{} is not a vector index; NEAREST requires a vector index.",
                    i
                ),
                None => "Vector search (NEAREST) requires a vector index (table.vectorindex)."
                    .to_string(),
            },
        );
    }
    let resolved = get_key_schema(client, &cmd.table, cmd.index.as_deref().unwrap_or(""))?;
    let pk = &resolved.pk;
    let rk = &resolved.rk;
    let has_index = cmd.index.is_some();
    let where_ = cmd.where_.as_ref();
    let can_getitem = !has_index && where_.map(|w| w.can_getitem(pk, rk)).unwrap_or(false);
    let qplan = where_.and_then(|w| w.plan_query(pk, rk));

    let mut st = SymbolTable::new(alias_prefix);
    let mut plan = SelectPlan::default();
    if can_getitem {
        let w = where_.unwrap();
        plan.api = SelectApi::GetItem;
        let r = &mut plan.get_item;
        r.table_name = cmd.table.clone();
        r.consistent_read = cmd.consistent;
        if !cmd.projection.is_empty() {
            r.projection_expression = Some(serialize_projection(&cmd.projection, &mut st));
        }
        r.expression_attribute_names = st.names_vec();
        r.return_consumed_capacity = cmd.rcc;
        r.key = key_of(&w.get_gud_key(pk, rk));
        return Ok(plan);
    }
    let rcc = if cmd.ratelimit.is_some() {
        ReturnConsumedCapacity::Total
    } else {
        cmd.rcc
    };
    if let Some(qp) = qplan {
        let w = where_.unwrap();
        plan.api = SelectApi::Query;
        let r = &mut plan.query;
        r.table_name = cmd.table.clone();
        if has_index {
            r.index_name = cmd.index.clone();
        }
        r.consistent_read = cmd.consistent;
        r.return_consumed_capacity = rcc;
        if !cmd.projection.is_empty() {
            r.projection_expression = Some(serialize_projection(&cmd.projection, &mut st));
        }
        r.key_condition_expression = w.query_key_condition_expression(&qp, Some(&mut st));
        let filter = w.query_filter_expression(&qp, Some(&mut st));
        if !filter.is_empty() {
            r.filter_expression = Some(filter);
        }
        r.expression_attribute_names = st.names_vec();
        r.expression_attribute_values = st.values_vec();
        return Ok(plan);
    }
    plan.api = SelectApi::Scan;
    let r = &mut plan.scan;
    r.table_name = cmd.table.clone();
    if has_index {
        r.index_name = cmd.index.clone();
    }
    r.consistent_read = cmd.consistent;
    r.return_consumed_capacity = rcc;
    if !cmd.projection.is_empty() {
        r.projection_expression = Some(serialize_projection(&cmd.projection, &mut st));
    }
    if let Some(w) = where_ {
        let filter = w.scan_filter_expression(Some(&mut st));
        if !filter.is_empty() {
            r.filter_expression = Some(filter);
        }
    }
    r.expression_attribute_names = st.names_vec();
    r.expression_attribute_values = st.values_vec();
    Ok(plan)
}
