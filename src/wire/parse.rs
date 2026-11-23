// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! Response parsing into the client-seam `*Out` structs (port of wire/parse.cpp).

use super::datetime::epoch_to_iso8601;
use super::json::{self, malformed, JsonValue};
use crate::client::*;
use crate::error::{Errc, Error, Result};
use crate::table_schema::*;
use crate::value::{Value, ValueMap};

fn parse_item(obj: &JsonValue) -> Result<ValueMap> {
    let members = obj
        .members()
        .ok_or_else(|| malformed("expected an item object"))?;
    members
        .iter()
        .map(|(k, v)| Ok((k.clone(), parse_value(v)?)))
        .collect()
}

fn parse_string_array(arr: &JsonValue, which: &str) -> Result<Vec<String>> {
    let elements = arr
        .as_array()
        .ok_or_else(|| malformed(&format!("expected an array for {}", which)))?;
    elements
        .iter()
        .map(|e| {
            e.as_str()
                .map(str::to_string)
                .ok_or_else(|| malformed(&format!("expected a string element in {}", which)))
        })
        .collect()
}

/// Decode one DynamoDB AttributeValue (`{"S":"x"}` etc).
pub fn parse_value(node: &JsonValue) -> Result<Value> {
    let members = match node.members() {
        Some(m) if m.len() == 1 => m,
        _ => return Err(malformed("attribute value must be a single-key object")),
    };
    let (tag, body) = (&members[0].0, &members[0].1);
    let need_str = |what: &str| {
        body.as_str()
            .map(str::to_string)
            .ok_or_else(|| malformed(what))
    };
    match tag.as_str() {
        "S" => Ok(Value::S(need_str("S must be a string")?)),
        "N" => Ok(Value::N(need_str("N must be a string")?)),
        "B" => Ok(Value::B(need_str("B must be a string")?)),
        "BOOL" => body
            .as_bool()
            .map(Value::Bool)
            .ok_or_else(|| malformed("BOOL must be a boolean")),
        "NULL" => body
            .as_bool()
            .map(|_| Value::Null)
            .ok_or_else(|| malformed("NULL must be a boolean")),
        "L" => {
            let elements = body
                .as_array()
                .ok_or_else(|| malformed("L must be an array"))?;
            Ok(Value::L(
                elements.iter().map(parse_value).collect::<Result<_>>()?,
            ))
        }
        "M" => {
            if !body.is_object() {
                return Err(malformed("M must be an object"));
            }
            Ok(Value::M(parse_item(body)?))
        }
        "SS" => Ok(Value::SS(parse_string_array(body, "SS")?)),
        "NS" => Ok(Value::NS(parse_string_array(body, "NS")?)),
        "BS" => Ok(Value::BS(parse_string_array(body, "BS")?)),
        other => Err(malformed(&format!(
            "unknown attribute value type '{}'",
            other
        ))),
    }
}

fn parse_capacity_double(node: &JsonValue, which: &str) -> Result<f64> {
    let tok = node
        .as_number()
        .ok_or_else(|| malformed(&format!("{} must be a number", which)))?;
    tok.parse::<f64>()
        .map_err(|_| malformed(&format!("invalid {} number", which)))
}

fn parse_consumed_capacity(root: &JsonValue) -> Result<Option<ConsumedCapacity>> {
    let cc = match root.find("ConsumedCapacity") {
        None => return Ok(None),
        Some(cc) => cc,
    };
    if !cc.is_object() {
        return Err(malformed("ConsumedCapacity must be an object"));
    }
    let mut out = ConsumedCapacity::default();
    let mut any = false;
    if let Some(tn) = cc.find("TableName") {
        out.table_name = Some(
            tn.as_str()
                .ok_or_else(|| malformed("ConsumedCapacity.TableName must be a string"))?
                .to_string(),
        );
        any = true;
    }
    if let Some(x) = cc.find("CapacityUnits") {
        out.capacity_units = Some(parse_capacity_double(x, "ConsumedCapacity.CapacityUnits")?);
        any = true;
    }
    if let Some(x) = cc.find("ReadCapacityUnits") {
        out.read_capacity_units = Some(parse_capacity_double(
            x,
            "ConsumedCapacity.ReadCapacityUnits",
        )?);
        any = true;
    }
    if let Some(x) = cc.find("WriteCapacityUnits") {
        out.write_capacity_units = Some(parse_capacity_double(
            x,
            "ConsumedCapacity.WriteCapacityUnits",
        )?);
        any = true;
    }
    Ok(if any { Some(out) } else { None })
}

fn exception_name_from_type(type_name: &str) -> String {
    match type_name.rfind('#') {
        Some(i) => type_name[i + 1..].to_string(),
        None => type_name.to_string(),
    }
}

fn error_from_type_document(root: &JsonValue) -> Error {
    let type_name = root
        .find("__type")
        .and_then(JsonValue::as_str)
        .unwrap_or("");
    let name = exception_name_from_type(type_name);
    let message = root
        .find("message")
        .and_then(JsonValue::as_str)
        .or_else(|| root.find("Message").and_then(JsonValue::as_str))
        .unwrap_or("")
        .to_string();
    let code = if name == "ConditionalCheckFailedException" {
        Errc::ConditionalCheckFailed
    } else {
        Errc::ServiceError
    };
    Error {
        code,
        message,
        service_error_name: Some(name),
        request_id: None,
    }
}

fn parse_response_object(body: &str) -> Result<JsonValue> {
    let doc = json::parse(body)?;
    if !doc.is_object() {
        return Err(malformed("expected a top-level object"));
    }
    if doc.find("__type").is_some() {
        return Err(error_from_type_document(&doc));
    }
    Ok(doc)
}

/// The exception short name (`__type` after `#`) of an error document, if any.
pub fn error_type_name(body: &str) -> Option<String> {
    let doc = json::parse(body).ok()?;
    doc.find("__type")
        .and_then(JsonValue::as_str)
        .map(exception_name_from_type)
}

fn parse_optional_item(root: &JsonValue, member: &str) -> Result<Option<ItemRow>> {
    let m = match root.find(member) {
        None => return Ok(None),
        Some(m) => m,
    };
    let members = m
        .members()
        .ok_or_else(|| malformed(&format!("{} must be an object", member)))?;
    if members.is_empty() {
        return Ok(None);
    }
    Ok(Some(ItemRow {
        attributes: parse_item(m)?,
    }))
}

fn parse_items(root: &JsonValue) -> Result<Vec<ItemRow>> {
    match root.find("Items") {
        None => Ok(vec![]),
        Some(items) => items
            .as_array()
            .ok_or_else(|| malformed("Items must be an array"))?
            .iter()
            .map(|e| {
                Ok(ItemRow {
                    attributes: parse_item(e)?,
                })
            })
            .collect(),
    }
}

fn parse_last_evaluated_key(root: &JsonValue) -> Result<Option<ValueMap>> {
    let lek = match root.find("LastEvaluatedKey") {
        None => return Ok(None),
        Some(l) => l,
    };
    let members = lek
        .members()
        .ok_or_else(|| malformed("LastEvaluatedKey must be an object"))?;
    if members.is_empty() {
        return Ok(None);
    }
    Ok(Some(parse_item(lek)?))
}

pub fn parse_get_item(body: &str) -> Result<GetItemOut> {
    let root = parse_response_object(body)?;
    Ok(GetItemOut {
        item: parse_optional_item(&root, "Item")?,
        consumed_capacity: parse_consumed_capacity(&root)?,
    })
}

pub fn parse_query(body: &str) -> Result<QueryOut> {
    let root = parse_response_object(body)?;
    Ok(QueryOut {
        items: parse_items(&root)?,
        last_evaluated_key: parse_last_evaluated_key(&root)?,
        consumed_capacity: parse_consumed_capacity(&root)?,
    })
}

pub fn parse_scan(body: &str) -> Result<ScanOut> {
    let q = parse_query(body)?;
    Ok(ScanOut {
        items: q.items,
        last_evaluated_key: q.last_evaluated_key,
        consumed_capacity: q.consumed_capacity,
    })
}

fn parse_write(body: &str) -> Result<(Option<ItemRow>, Option<ConsumedCapacity>)> {
    let root = parse_response_object(body)?;
    Ok((
        parse_optional_item(&root, "Attributes")?,
        parse_consumed_capacity(&root)?,
    ))
}

pub fn parse_put_item(body: &str) -> Result<PutItemOut> {
    let (attributes, consumed_capacity) = parse_write(body)?;
    Ok(PutItemOut {
        attributes,
        consumed_capacity,
    })
}
pub fn parse_update_item(body: &str) -> Result<UpdateItemOut> {
    let (attributes, consumed_capacity) = parse_write(body)?;
    Ok(UpdateItemOut {
        attributes,
        consumed_capacity,
    })
}
pub fn parse_delete_item(body: &str) -> Result<DeleteItemOut> {
    let (attributes, consumed_capacity) = parse_write(body)?;
    Ok(DeleteItemOut {
        attributes,
        consumed_capacity,
    })
}

// ---- control plane helpers ------------------------------------------------------

fn str_field(obj: &JsonValue, key: &str) -> String {
    obj.find(key)
        .and_then(JsonValue::as_str)
        .unwrap_or("")
        .to_string()
}
fn bool_field(obj: &JsonValue, key: &str) -> bool {
    obj.find(key).and_then(JsonValue::as_bool).unwrap_or(false)
}
fn ll_field(obj: &JsonValue, key: &str, dflt: i64) -> i64 {
    obj.find(key)
        .and_then(JsonValue::as_number)
        .and_then(|t| t.parse::<i64>().ok())
        .unwrap_or(dflt)
}
fn double_field(obj: &JsonValue, key: &str, dflt: f64) -> f64 {
    obj.find(key)
        .and_then(JsonValue::as_number)
        .and_then(|t| t.parse::<f64>().ok())
        .unwrap_or(dflt)
}
fn iso_datetime_field(obj: &JsonValue, key: &str) -> String {
    epoch_to_iso8601(double_field(obj, key, 0.0))
}
fn object<'a>(obj: &'a JsonValue, key: &str) -> Option<&'a JsonValue> {
    obj.find(key).filter(|v| v.is_object())
}
fn array<'a>(obj: &'a JsonValue, key: &str) -> &'a [JsonValue] {
    obj.find(key).and_then(JsonValue::as_array).unwrap_or(&[])
}

fn load_key_schema(arr: Option<&JsonValue>) -> TableKeySchema {
    let mut out = TableKeySchema::default();
    for e in arr.and_then(JsonValue::as_array).unwrap_or(&[]) {
        let name = str_field(e, "AttributeName");
        match str_field(e, "KeyType").as_str() {
            "HASH" => out.hash_keys.push(name),
            "RANGE" => out.range_keys.push(name),
            _ => {}
        }
    }
    out
}

fn load_projection(obj: &JsonValue) -> Projection {
    let mut out = Projection::default();
    match str_field(obj, "ProjectionType").as_str() {
        "KEYS_ONLY" => out.ty = ProjectionType::KeysOnly,
        "INCLUDE" => {
            out.ty = ProjectionType::Include;
            out.non_key_attributes = array(obj, "NonKeyAttributes")
                .iter()
                .filter_map(|a| a.as_str().map(str::to_string))
                .collect();
        }
        _ => out.ty = ProjectionType::All,
    }
    out
}

fn load_scalar(t: &str) -> ScalarType {
    match t {
        "N" => ScalarType::N,
        "B" => ScalarType::B,
        _ => ScalarType::S,
    }
}

fn load_warm(obj: &JsonValue) -> Option<WarmThroughputDescription> {
    object(obj, "WarmThroughput").map(|w| WarmThroughputDescription {
        read_units_per_second: ll_field(w, "ReadUnitsPerSecond", 0),
        write_units_per_second: ll_field(w, "WriteUnitsPerSecond", 0),
        status: str_field(w, "Status"),
    })
}

fn load_gsi_detail(gsi: &JsonValue) -> SecondaryIndexSchema {
    let mut out = SecondaryIndexSchema {
        index_name: str_field(gsi, "IndexName"),
        key_schema: load_key_schema(gsi.find("KeySchema")),
        ..Default::default()
    };
    if let Some(p) = object(gsi, "Projection") {
        out.projection = load_projection(p);
        out.projection_set = true;
    }
    out.index_status = str_field(gsi, "IndexStatus");
    out.backfilling = bool_field(gsi, "Backfilling");
    if let Some(pt) = object(gsi, "ProvisionedThroughput") {
        out.provisioned_read_capacity_units = ll_field(pt, "ReadCapacityUnits", 0);
        out.provisioned_write_capacity_units = ll_field(pt, "WriteCapacityUnits", 0);
    }
    if let Some(od) = object(gsi, "OnDemandThroughput") {
        out.on_demand_throughput_set = true;
        out.on_demand_max_read_request_units = ll_field(od, "MaxReadRequestUnits", -1);
        out.on_demand_max_write_request_units = ll_field(od, "MaxWriteRequestUnits", -1);
    }
    out.warm_throughput = load_warm(gsi);
    out
}

fn load_vector_index_detail(vi: &JsonValue) -> VectorIndexSchema {
    let mut out = VectorIndexSchema {
        index_name: str_field(vi, "IndexName"),
        ..Default::default()
    };
    if let Some(va) = object(vi, "VectorAttribute") {
        out.vector_attr = str_field(va, "AttributeName");
    }
    out.dimensions = ll_field(vi, "Dimensions", 0);
    out.distance = match str_field(vi, "DistanceFunction").as_str() {
        "EUCLIDEAN" => VectorDistance::Euclidean,
        "DOT_PRODUCT" => VectorDistance::DotProduct,
        _ => VectorDistance::Cosine,
    };
    for se in array(vi, "SearchSchema") {
        let kind = if str_field(se, "SearchSchemaElementType") == "HASH" {
            SearchSchemaKind::Hash
        } else {
            SearchSchemaKind::InlineFilter
        };
        out.search_schema.push(SearchSchemaElem {
            attr: str_field(se, "AttributeName"),
            kind,
        });
    }
    if let Some(p) = object(vi, "Projection") {
        out.projection = load_projection(p);
        out.projection_set = true;
    }
    out.index_status = str_field(vi, "IndexStatus");
    out
}

/// Acknowledgement-only responses: success unless an error document.
pub fn parse_ack(body: &str) -> Result<()> {
    parse_response_object(body).map(|_| ())
}

pub fn parse_describe_table(body: &str) -> Result<DescribeTableOut> {
    let root = parse_response_object(body)?;
    let table =
        object(&root, "Table").ok_or_else(|| malformed("DescribeTable response missing Table"))?;
    let mut out = DescribeTableOut {
        table_name: str_field(table, "TableName"),
        key_schema: load_key_schema(table.find("KeySchema")),
        ..Default::default()
    };
    for lsi in array(table, "LocalSecondaryIndexes") {
        out.local_secondary_indexes.push(SecondaryIndexSchema {
            index_name: str_field(lsi, "IndexName"),
            key_schema: load_key_schema(lsi.find("KeySchema")),
            ..Default::default()
        });
    }
    out.global_secondary_indexes = array(table, "GlobalSecondaryIndexes")
        .iter()
        .map(load_gsi_detail)
        .collect();
    out.vector_indexes = array(table, "VectorIndexes")
        .iter()
        .map(load_vector_index_detail)
        .collect();
    out.table_status = str_field(table, "TableStatus");
    out.table_id = str_field(table, "TableId");
    out.table_arn = str_field(table, "TableArn");
    if let Some(bms) = object(table, "BillingModeSummary") {
        out.billing_mode_summary = Some(str_field(bms, "BillingMode"));
    }
    out.provisioned_throughput_set = table.find("ProvisionedThroughput").is_some();
    if let Some(tcs) = object(table, "TableClassSummary") {
        out.table_class_summary = Some(str_field(tcs, "TableClass"));
    }
    out.attribute_definitions = array(table, "AttributeDefinitions")
        .iter()
        .map(|ad| AttributeDef {
            name: str_field(ad, "AttributeName"),
            ty: load_scalar(&str_field(ad, "AttributeType")),
        })
        .collect();
    out.creation_date_time = iso_datetime_field(table, "CreationDateTime");
    out.table_size_bytes = ll_field(table, "TableSizeBytes", 0);
    out.item_count = ll_field(table, "ItemCount", 0);
    out.deletion_protection_enabled = bool_field(table, "DeletionProtectionEnabled");
    if let Some(pt) = object(table, "ProvisionedThroughput") {
        out.provisioned_read_capacity_units = ll_field(pt, "ReadCapacityUnits", 0);
        out.provisioned_write_capacity_units = ll_field(pt, "WriteCapacityUnits", 0);
    }
    if let Some(od) = object(table, "OnDemandThroughput") {
        out.on_demand_throughput_set = true;
        out.on_demand_max_read_request_units = ll_field(od, "MaxReadRequestUnits", -1);
        out.on_demand_max_write_request_units = ll_field(od, "MaxWriteRequestUnits", -1);
    }
    out.warm_throughput = load_warm(table);
    if let Some(ss) = object(table, "StreamSpecification") {
        out.stream_specification_set = true;
        if bool_field(ss, "StreamEnabled") {
            out.stream_view_type = Some(str_field(ss, "StreamViewType"));
        }
    }
    if let Some(sse) = object(table, "SSEDescription") {
        out.sse_status = Some(str_field(sse, "Status"));
    }
    Ok(out)
}

pub fn parse_describe_time_to_live(body: &str) -> Result<TimeToLiveOut> {
    let root = parse_response_object(body)?;
    let mut out = TimeToLiveOut::default();
    if let Some(d) = object(&root, "TimeToLiveDescription") {
        out.status = str_field(d, "TimeToLiveStatus");
        out.attribute_name = str_field(d, "AttributeName");
    }
    Ok(out)
}

pub fn parse_describe_continuous_backups(body: &str) -> Result<ContinuousBackupsOut> {
    let root = parse_response_object(body)?;
    let mut out = ContinuousBackupsOut::default();
    if let Some(pitr) = object(&root, "ContinuousBackupsDescription")
        .and_then(|c| object(c, "PointInTimeRecoveryDescription"))
    {
        out.pitr_status = str_field(pitr, "PointInTimeRecoveryStatus");
        if out.pitr_status == "ENABLED" {
            out.earliest_restorable_date_time =
                iso_datetime_field(pitr, "EarliestRestorableDateTime");
            out.latest_restorable_date_time = iso_datetime_field(pitr, "LatestRestorableDateTime");
        }
    }
    Ok(out)
}

pub fn parse_describe_table_replica_auto_scaling(body: &str) -> Result<TableReplicaAutoScalingOut> {
    let root = parse_response_object(body)?;
    let mut out = TableReplicaAutoScalingOut::default();
    if let Some(asd) = object(&root, "TableAutoScalingDescription") {
        out.replicas = array(asd, "Replicas")
            .iter()
            .map(|r| ReplicaAutoScalingDescription {
                region_name: str_field(r, "RegionName"),
                replica_status: str_field(r, "ReplicaStatus"),
            })
            .collect();
    }
    Ok(out)
}

pub fn parse_describe_limits(body: &str) -> Result<DescribeLimitsOut> {
    let root = parse_response_object(body)?;
    Ok(DescribeLimitsOut {
        account_max_rcu: ll_field(&root, "AccountMaxReadCapacityUnits", 0),
        account_max_wcu: ll_field(&root, "AccountMaxWriteCapacityUnits", 0),
        table_max_rcu: ll_field(&root, "TableMaxReadCapacityUnits", 0),
        table_max_wcu: ll_field(&root, "TableMaxWriteCapacityUnits", 0),
    })
}

pub fn parse_describe_backup(body: &str) -> Result<DescribeBackupOut> {
    let root = parse_response_object(body)?;
    let bd = object(&root, "BackupDescription")
        .ok_or_else(|| malformed("DescribeBackup response missing BackupDescription"))?;
    let mut out = DescribeBackupOut::default();
    if let Some(sd) = object(bd, "SourceTableDetails") {
        out.source_table_name = str_field(sd, "TableName");
        out.source_table_id = str_field(sd, "TableId");
        out.source_table_arn = str_field(sd, "TableArn");
        out.table_size_bytes = ll_field(sd, "TableSizeBytes", 0);
        out.item_count = ll_field(sd, "ItemCount", 0);
        out.key_schema = array(sd, "KeySchema")
            .iter()
            .map(|e| BackupKeySchemaElement {
                key_type: str_field(e, "KeyType"),
                attribute_name: str_field(e, "AttributeName"),
            })
            .collect();
        out.table_creation_datetime = iso_datetime_field(sd, "TableCreationDateTime");
        if sd.find("BillingMode").is_some() {
            out.billing_mode = Some(str_field(sd, "BillingMode"));
        }
        if let Some(pt) = object(sd, "ProvisionedThroughput") {
            out.provisioned = Some((
                ll_field(pt, "ReadCapacityUnits", 0),
                ll_field(pt, "WriteCapacityUnits", 0),
            ));
        }
    }
    if let Some(det) = object(bd, "BackupDetails") {
        out.backup_name = str_field(det, "BackupName");
        out.backup_arn = str_field(det, "BackupArn");
        out.backup_size_bytes = ll_field(det, "BackupSizeBytes", 0);
        out.backup_status = str_field(det, "BackupStatus");
        out.backup_type = str_field(det, "BackupType");
        out.backup_creation_datetime = iso_datetime_field(det, "BackupCreationDateTime");
        out.backup_expiry_datetime = iso_datetime_field(det, "BackupExpiryDateTime");
    }
    Ok(out)
}

pub fn parse_list_backups(body: &str) -> Result<ListBackupsOut> {
    let root = parse_response_object(body)?;
    let summaries = array(&root, "BackupSummaries")
        .iter()
        .map(|s| BackupSummaryRow {
            table_name: str_field(s, "TableName"),
            backup_name: str_field(s, "BackupName"),
            backup_status: str_field(s, "BackupStatus"),
            backup_arn: str_field(s, "BackupArn"),
            creation_datetime: iso_datetime_field(s, "BackupCreationDateTime"),
            expiry_datetime: iso_datetime_field(s, "BackupExpiryDateTime"),
        })
        .collect();
    let cursor = str_field(&root, "LastEvaluatedBackupArn");
    Ok(ListBackupsOut {
        summaries,
        last_evaluated_backup_arn: if cursor.is_empty() {
            None
        } else {
            Some(cursor)
        },
    })
}

pub fn parse_search_vectors(body: &str) -> Result<SearchVectorsOut> {
    let root = parse_response_object(body)?;
    let mut out = SearchVectorsOut::default();
    for r in array(&root, "SearchResults") {
        let mut row = SearchVectorRow::default();
        if let Some(item) = object(r, "Item") {
            row.item = ItemRow {
                attributes: parse_item(item)?,
            };
        }
        row.score = double_field(r, "Score", 0.0);
        out.results.push(row);
    }
    let cc = object(&root, "ConsumedCapacity");
    out.vector_capacity = Some(VectorConsumedCapacity {
        vector_search_request_bytes: Some(
            cc.map(|c| double_field(c, "VectorSearchRequestBytes", 0.0))
                .unwrap_or(0.0),
        ),
        vector_write_request_bytes: Some(
            cc.map(|c| double_field(c, "VectorWriteRequestBytes", 0.0))
                .unwrap_or(0.0),
        ),
    });
    Ok(out)
}

pub fn parse_transact_get_items(body: &str) -> Result<TransactGetItemsOut> {
    let root = parse_response_object(body)?;
    let mut out = TransactGetItemsOut::default();
    for r in array(&root, "Responses") {
        match r.find("Item").and_then(JsonValue::members) {
            Some(m) if !m.is_empty() => out.responses.push(Some(ItemRow {
                attributes: parse_item(r.find("Item").unwrap())?,
            })),
            _ => out.responses.push(None),
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ListTablesPage {
    pub table_names: Vec<String>,
    pub last_evaluated_table_name: Option<String>,
}

pub fn parse_list_tables(body: &str) -> Result<ListTablesPage> {
    let root = parse_response_object(body)?;
    let table_names = array(&root, "TableNames")
        .iter()
        .filter_map(|n| n.as_str().map(str::to_string))
        .collect();
    let cursor = str_field(&root, "LastEvaluatedTableName");
    Ok(ListTablesPage {
        table_names,
        last_evaluated_table_name: if cursor.is_empty() {
            None
        } else {
            Some(cursor)
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_item_variants() {
        let out = parse_get_item(r#"{"Item":{"id":{"S":"k1"},"v":{"N":"7"}}}"#).unwrap();
        assert_eq!(out.item.unwrap().attributes.len(), 2);
        assert!(out.consumed_capacity.is_none());
        assert!(parse_get_item("{}").unwrap().item.is_none());
        assert!(parse_get_item(r#"{"Item":{}}"#).unwrap().item.is_none());
    }

    #[test]
    fn query_items_and_lek() {
        let out = parse_query(
            r#"{"Items":[{"id":{"S":"a"}},{"id":{"S":"b"}}],"LastEvaluatedKey":{"id":{"S":"b"}},"Count":2,"ScannedCount":5}"#,
        )
        .unwrap();
        assert_eq!(out.items.len(), 2);
        assert_eq!(out.last_evaluated_key.unwrap().len(), 1);
        assert!(parse_scan(r#"{"Items":[],"LastEvaluatedKey":{}}"#)
            .unwrap()
            .last_evaluated_key
            .is_none());
    }

    #[test]
    fn consumed_capacity_semantics() {
        let out = parse_scan(
            r#"{"Items":[],"ConsumedCapacity":{"TableName":"t","CapacityUnits":3.5,"ReadCapacityUnits":2.0,"WriteCapacityUnits":1.5}}"#,
        )
        .unwrap();
        let cc = out.consumed_capacity.unwrap();
        assert_eq!(cc.table_name.as_deref(), Some("t"));
        assert_eq!(cc.capacity_units, Some(3.5));
        assert_eq!(cc.write_capacity_units, Some(1.5));
        assert!(parse_query(r#"{"Items":[],"ConsumedCapacity":{}}"#)
            .unwrap()
            .consumed_capacity
            .is_none());
        let only = parse_query(r#"{"Items":[],"ConsumedCapacity":{"CapacityUnits":1}}"#).unwrap();
        assert_eq!(only.consumed_capacity.unwrap().capacity_units, Some(1.0));
    }

    #[test]
    fn all_value_types() {
        let out = parse_get_item(
            r#"{"Item":{"s":{"S":"x"},"n":{"N":"1.5"},"b":{"B":"AQI="},"t":{"BOOL":true},"z":{"NULL":true},
            "l":{"L":[{"N":"1"}]},"m":{"M":{"k":{"S":"v"}}},"ss":{"SS":["a"]},"ns":{"NS":["1"]},"bs":{"BS":["AQ=="]}}}"#,
        )
        .unwrap();
        let a = out.item.unwrap().attributes;
        assert_eq!(a[0].1, Value::S("x".into()));
        assert_eq!(a[4].1, Value::Null);
        assert_eq!(a[6].1, Value::M(vec![("k".into(), Value::S("v".into()))]));
        assert_eq!(a[9].1, Value::BS(vec!["AQ==".into()]));
    }

    #[test]
    fn error_documents() {
        let e = parse_put_item(
            r#"{"__type":"com.amazonaws.dynamodb.v20120810#ConditionalCheckFailedException","message":"The conditional request failed"}"#,
        )
        .unwrap_err();
        assert_eq!(e.code, Errc::ConditionalCheckFailed);
        assert_eq!(e.message, "The conditional request failed");
        assert_eq!(
            e.service_error_name.as_deref(),
            Some("ConditionalCheckFailedException")
        );
        let e = parse_describe_table(r#"{"__type":"com.amazonaws.dynamodb.v20120810#ResourceNotFoundException","Message":"Requested resource not found"}"#)
            .unwrap_err();
        assert_eq!(e.code, Errc::ServiceError);
        assert_eq!(
            e.service_error_name.as_deref(),
            Some("ResourceNotFoundException")
        );
        assert_eq!(e.message, "Requested resource not found");
        assert_eq!(
            error_type_name(r#"{"__type":"x#ThrottlingException"}"#).as_deref(),
            Some("ThrottlingException")
        );
        assert_eq!(error_type_name("{}"), None);
    }

    #[test]
    fn malformed_values() {
        assert!(parse_get_item(r#"{"Item":{"a":{"S":1}}}"#)
            .unwrap_err()
            .message
            .contains("S must be a string"));
        assert!(parse_get_item(r#"{"Item":{"a":{"X":"1"}}}"#)
            .unwrap_err()
            .message
            .contains("unknown attribute value type 'X'"));
        assert!(parse_get_item(r#"[]"#)
            .unwrap_err()
            .message
            .contains("expected a top-level object"));
    }

    #[test]
    fn describe_table_fields() {
        let body = r#"{"Table":{"TableName":"t","TableStatus":"ACTIVE","KeySchema":[{"AttributeName":"a","KeyType":"HASH"},{"AttributeName":"b","KeyType":"RANGE"}],
          "AttributeDefinitions":[{"AttributeName":"a","AttributeType":"N"}],"CreationDateTime":1.7e9,"ItemCount":3,
          "BillingModeSummary":{"BillingMode":"PAY_PER_REQUEST"},"ProvisionedThroughput":{"ReadCapacityUnits":0,"WriteCapacityUnits":0},
          "GlobalSecondaryIndexes":[{"IndexName":"g","KeySchema":[{"AttributeName":"c","KeyType":"HASH"}],"Projection":{"ProjectionType":"INCLUDE","NonKeyAttributes":["x"]},"IndexStatus":"ACTIVE","OnDemandThroughput":{"MaxReadRequestUnits":5}}],
          "StreamSpecification":{"StreamEnabled":true,"StreamViewType":"NEW_IMAGE"}}}"#;
        let d = parse_describe_table(body).unwrap();
        assert_eq!(d.key_schema.range_keys, vec!["b".to_string()]);
        assert_eq!(d.creation_date_time, "2023-11-14T22:13:20Z");
        assert_eq!(d.item_count, 3);
        assert!(d.provisioned_throughput_set);
        assert_eq!(d.attribute_definitions[0].ty, ScalarType::N);
        let g = &d.global_secondary_indexes[0];
        assert_eq!(g.projection.non_key_attributes, vec!["x".to_string()]);
        assert_eq!(g.on_demand_max_read_request_units, 5);
        assert_eq!(g.on_demand_max_write_request_units, -1);
        assert_eq!(d.stream_view_type.as_deref(), Some("NEW_IMAGE"));
        assert!(parse_describe_table("{}")
            .unwrap_err()
            .message
            .contains("missing Table"));
    }

    #[test]
    fn list_tables_and_transact_get() {
        let p =
            parse_list_tables(r#"{"TableNames":["a","b"],"LastEvaluatedTableName":"b"}"#).unwrap();
        assert_eq!(p.table_names.len(), 2);
        assert_eq!(p.last_evaluated_table_name.as_deref(), Some("b"));
        let t =
            parse_transact_get_items(r#"{"Responses":[{"Item":{"a":{"N":"1"}}},{},{"Item":{}}]}"#)
                .unwrap();
        assert_eq!(t.responses.len(), 3);
        assert!(t.responses[0].is_some() && t.responses[1].is_none() && t.responses[2].is_none());
    }
}
