//! Request serialization to the exact AWS-SDK `SerializePayload()` JSON body
//! (tab-indented; one tab per level and a tab after each key's colon), and
//! the `%1.15g`/`%1.17g` cJSON number rule (port of wire/codec.cpp).

use super::datetime::iso8601_to_epoch;
use crate::client::*;
use crate::table_schema::*;
use crate::value::{Value, ValueList, ValueMap};

#[derive(Debug, Clone, PartialEq)]
pub struct WireRequest {
    pub operation: String,
    pub body: String,
}

fn req(op: &str, root: &Json) -> WireRequest {
    WireRequest {
        operation: op.to_string(),
        body: render(root),
    }
}

// ---- JSON writer -----------------------------------------------------------

#[derive(Debug, Clone)]
enum Json {
    Object(Vec<(String, Json)>),
    Array(Vec<Json>),
    Str(String),
    Bool(bool),
    Number(String),
}

impl Json {
    fn obj() -> Json {
        Json::Object(Vec::new())
    }
    fn put(&mut self, key: &str, value: Json) {
        if let Json::Object(m) = self {
            m.push((key.to_string(), value));
        }
    }
    fn sort_members(&mut self) {
        if let Json::Object(m) = self {
            m.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        }
    }
}

fn s(v: &str) -> Json {
    Json::Str(v.to_string())
}
fn b(v: bool) -> Json {
    Json::Bool(v)
}
fn int(n: i64) -> Json {
    Json::Number(n.to_string())
}

/// C `%.<precision>g` formatting.
pub(crate) fn format_g(d: f64, precision: usize) -> String {
    if d == 0.0 {
        return if d.is_sign_negative() {
            "-0".into()
        } else {
            "0".into()
        };
    }
    let p = precision.max(1);
    let sci = format!("{:.*e}", p - 1, d);
    let (mant, exp) = sci.split_once('e').unwrap();
    let x: i32 = exp.parse().unwrap();
    let strip = |t: &str| -> String {
        if t.contains('.') {
            t.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            t.to_string()
        }
    };
    if x < -4 || x >= p as i32 {
        let m = strip(mant);
        let sign = if x < 0 { '-' } else { '+' };
        format!("{}e{}{:02}", m, sign, x.abs())
    } else {
        let decimals = (p as i32 - 1 - x).max(0) as usize;
        strip(&format!("{:.*}", decimals, d))
    }
}

/// cJSON's double printing: `%1.15g`, falling back to `%1.17g` when that
/// does not round-trip.
pub(crate) fn cjson_number_double(d: f64) -> String {
    if d.is_nan() || d.is_infinite() {
        return "null".into();
    }
    let short = format_g(d, 15);
    let recovered = short
        .parse::<f64>()
        .map(|t| (t - d).abs() <= d.abs().max(t.abs()) * f64::EPSILON)
        .unwrap_or(false);
    if recovered {
        short
    } else {
        format_g(d, 17)
    }
}

fn write_string(v: &str, out: &mut String) {
    out.push('"');
    for c in v.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

fn write_value(node: &Json, depth: usize, out: &mut String) {
    match node {
        Json::Str(v) => write_string(v, out),
        Json::Bool(v) => out.push_str(if *v { "true" } else { "false" }),
        Json::Number(v) => out.push_str(v),
        Json::Object(members) => {
            let inner = depth + 1;
            out.push_str("{\n");
            for (i, (k, v)) in members.iter().enumerate() {
                out.extend(std::iter::repeat_n('\t', inner));
                write_string(k, out);
                out.push_str(":\t");
                write_value(v, inner, out);
                if i + 1 < members.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.extend(std::iter::repeat_n('\t', inner - 1));
            out.push('}');
        }
        Json::Array(elements) => {
            let inner = depth + 1;
            out.push('[');
            for (i, e) in elements.iter().enumerate() {
                write_value(e, inner, out);
                if i + 1 < elements.len() {
                    out.push_str(", ");
                }
            }
            out.push(']');
        }
    }
}

fn render(root: &Json) -> String {
    let mut out = String::new();
    write_value(root, 0, &mut out);
    out
}

// ---- builders -----------------------------------------------------------------

fn av(v: &Value) -> Json {
    let mut o = Json::obj();
    match v {
        Value::S(x) => o.put("S", s(x)),
        Value::N(x) => o.put("N", s(x)),
        Value::B(x) => o.put("B", s(x)),
        Value::Bool(x) => o.put("BOOL", b(*x)),
        Value::Null | Value::Param(_) => o.put("NULL", b(true)),
        Value::L(list) => o.put("L", Json::Array(list.iter().map(av).collect())),
        Value::M(map) => {
            let mut m = Json::Object(map.iter().map(|(k, x)| (k.clone(), av(x))).collect());
            m.sort_members();
            o.put("M", m);
        }
        Value::SS(set) => o.put("SS", Json::Array(set.iter().map(|x| s(x)).collect())),
        Value::NS(set) => o.put("NS", Json::Array(set.iter().map(|x| s(x)).collect())),
        Value::BS(set) => o.put("BS", Json::Array(set.iter().map(|x| s(x)).collect())),
    }
    o
}

fn value_map_object(m: &ValueMap) -> Json {
    let mut o = Json::Object(m.iter().map(|(k, v)| (k.clone(), av(v))).collect());
    o.sort_members();
    o
}

fn names_object(names: &ExpressionAttributeNames) -> Json {
    let mut o = Json::Object(names.iter().map(|(t, n)| (t.clone(), s(n))).collect());
    o.sort_members();
    o
}

fn value_list_array(values: &ValueList) -> Json {
    Json::Array(values.iter().map(av).collect())
}

fn rcc_name(r: ReturnConsumedCapacity) -> &'static str {
    match r {
        ReturnConsumedCapacity::Total => "TOTAL",
        ReturnConsumedCapacity::Indexes => "INDEXES",
        ReturnConsumedCapacity::None => "NONE",
    }
}

fn rv_name(r: ReturnValue) -> &'static str {
    match r {
        ReturnValue::AllOld => "ALL_OLD",
        ReturnValue::AllNew => "ALL_NEW",
        ReturnValue::UpdatedOld => "UPDATED_OLD",
        ReturnValue::UpdatedNew => "UPDATED_NEW",
        ReturnValue::None => "NONE",
    }
}

fn scalar_type_name(t: ScalarType) -> &'static str {
    match t {
        ScalarType::N => "N",
        ScalarType::B => "B",
        ScalarType::S => "S",
    }
}

fn projection_type_name(t: ProjectionType) -> &'static str {
    match t {
        ProjectionType::KeysOnly => "KEYS_ONLY",
        ProjectionType::Include => "INCLUDE",
        ProjectionType::All => "ALL",
    }
}

fn table_class_name(t: TableClass) -> &'static str {
    match t {
        TableClass::StandardInfrequentAccess => "STANDARD_INFREQUENT_ACCESS",
        TableClass::Standard => "STANDARD",
    }
}

fn stream_view_type_name(v: StreamViewType) -> &'static str {
    match v {
        StreamViewType::NewImage => "NEW_IMAGE",
        StreamViewType::OldImage => "OLD_IMAGE",
        StreamViewType::NewAndOldImages => "NEW_AND_OLD_IMAGES",
        StreamViewType::KeysOnly => "KEYS_ONLY",
    }
}

fn vector_distance_name(d: VectorDistance) -> &'static str {
    match d {
        VectorDistance::Euclidean => "EUCLIDEAN",
        VectorDistance::DotProduct => "DOT_PRODUCT",
        VectorDistance::Cosine => "COSINE",
    }
}

fn key_schema_array(ks: &KeySchema) -> Json {
    let mut a = Vec::new();
    for (names, kt) in [(&ks.hash, "HASH"), (&ks.range, "RANGE")] {
        for name in names {
            let mut e = Json::obj();
            e.put("AttributeName", s(name));
            e.put("KeyType", s(kt));
            a.push(e);
        }
    }
    Json::Array(a)
}

fn projection_object(p: &Projection) -> Json {
    let mut o = Json::obj();
    o.put("ProjectionType", s(projection_type_name(p.ty)));
    if p.ty == ProjectionType::Include && !p.non_key_attributes.is_empty() {
        o.put(
            "NonKeyAttributes",
            Json::Array(p.non_key_attributes.iter().map(|x| s(x)).collect()),
        );
    }
    o
}

fn provisioned_object(bm: &BillingModeAndThroughput) -> Json {
    let mut o = Json::obj();
    o.put("ReadCapacityUnits", int(bm.rcu));
    o.put("WriteCapacityUnits", int(bm.wcu));
    o
}

fn on_demand_object(od: &OnDemandThroughput) -> Json {
    let mut o = Json::obj();
    if let Some(r) = od.max_read_request_units {
        o.put("MaxReadRequestUnits", int(r));
    }
    if let Some(w) = od.max_write_request_units {
        o.put("MaxWriteRequestUnits", int(w));
    }
    o
}

fn warm_object(w: &WarmThroughput) -> Json {
    let mut o = Json::obj();
    o.put("ReadUnitsPerSecond", int(w.read_units_per_second));
    o.put("WriteUnitsPerSecond", int(w.write_units_per_second));
    o
}

fn stream_object(st: &StreamSpec) -> Json {
    let mut o = Json::obj();
    o.put("StreamEnabled", b(st.enabled));
    if st.enabled {
        o.put("StreamViewType", s(stream_view_type_name(st.view_type)));
    }
    o
}

fn sse_object(sp: &SseSpec) -> Json {
    let mut o = Json::obj();
    o.put("Enabled", b(sp.enabled));
    if sp.enabled {
        let t = match sp.sse_type.unwrap_or(SseType::Kms) {
            SseType::Aes256 => "AES256",
            SseType::Kms => "KMS",
        };
        o.put("SSEType", s(t));
        if let Some(k) = &sp.kms_master_key_id {
            o.put("KMSMasterKeyId", s(k));
        }
    }
    o
}

fn attribute_definitions_array(defs: &[AttributeDef]) -> Json {
    Json::Array(
        defs.iter()
            .map(|d| {
                let mut e = Json::obj();
                e.put("AttributeName", s(&d.name));
                e.put("AttributeType", s(scalar_type_name(d.ty)));
                e
            })
            .collect(),
    )
}

fn vector_index_object(v: &VectorIndexDef) -> Json {
    let mut o = Json::obj();
    o.put("IndexName", s(&v.index_name));
    let mut va = Json::obj();
    va.put("AttributeName", s(&v.vector_attr));
    o.put("VectorAttribute", va);
    if !v.search_schema.is_empty() {
        let arr = v
            .search_schema
            .iter()
            .map(|e| {
                let mut x = Json::obj();
                x.put("AttributeName", s(&e.attr));
                x.put(
                    "SearchSchemaElementType",
                    s(if e.kind == SearchSchemaKind::Hash {
                        "HASH"
                    } else {
                        "INLINE_FILTER"
                    }),
                );
                x
            })
            .collect();
        o.put("SearchSchema", Json::Array(arr));
    }
    o.put("Projection", projection_object(&v.projection));
    o.put("Dimensions", int(v.dimensions));
    o.put("DistanceFunction", s(vector_distance_name(v.distance)));
    o
}

fn put_billing(o: &mut Json, billing: &Option<BillingModeAndThroughput>) {
    if let Some(bm) = billing {
        if bm.mode == BillingMode::Provisioned {
            o.put("ProvisionedThroughput", provisioned_object(bm));
        } else {
            o.put("OnDemandThroughput", on_demand_object(&bm.od));
        }
    }
}

fn gsi_object(g: &Gsi) -> Json {
    let mut o = Json::obj();
    o.put("IndexName", s(&g.name));
    o.put("KeySchema", key_schema_array(&g.key_schema));
    o.put("Projection", projection_object(&g.projection));
    put_billing(&mut o, &g.billing);
    if let Some(w) = &g.warm {
        o.put("WarmThroughput", warm_object(w));
    }
    o
}

fn lsi_object(l: &Lsi) -> Json {
    let mut o = Json::obj();
    o.put("IndexName", s(&l.name));
    o.put("KeySchema", key_schema_array(&l.key_schema));
    o.put("Projection", projection_object(&l.projection));
    o
}

fn gsi_update_object(gu: &GsiUpdate) -> Json {
    let mut wrapper = Json::obj();
    let mut a = Json::obj();
    a.put("IndexName", s(&gu.index_name));
    match gu.action {
        GsiAction::Create => {
            if let Some(ks) = &gu.key_schema {
                a.put("KeySchema", key_schema_array(ks));
            }
            if let Some(p) = &gu.projection {
                a.put("Projection", projection_object(p));
            }
            put_billing(&mut a, &gu.billing);
            if let Some(w) = &gu.warm {
                a.put("WarmThroughput", warm_object(w));
            }
            wrapper.put("Create", a);
        }
        GsiAction::Update => {
            put_billing(&mut a, &gu.billing);
            if let Some(w) = &gu.warm {
                a.put("WarmThroughput", warm_object(w));
            }
            wrapper.put("Update", a);
        }
        GsiAction::Drop => wrapper.put("Delete", a),
    }
    wrapper
}

fn vector_index_update_object(u: &VectorIndexUpdateDef) -> Json {
    let mut wrapper = Json::obj();
    if let Some(c) = &u.create {
        wrapper.put("Create", vector_index_object(c));
    } else if let Some(d) = &u.drop {
        let mut a = Json::obj();
        a.put("IndexName", s(d));
        wrapper.put("Delete", a);
    }
    wrapper
}

fn throughput_override_object(o: &ProvisionedThroughputOverride) -> Json {
    let mut out = Json::obj();
    out.put("ReadCapacityUnits", int(o.read_capacity_units));
    out
}

fn replica_update_object(r: &ReplicaUpdate) -> Json {
    let mut wrapper = Json::obj();
    let build = |ru: &ReplicaUpdate| {
        let mut a = Json::obj();
        a.put("RegionName", s(&ru.region));
        if let Some(t) = &ru.throughput_override {
            a.put(
                "ProvisionedThroughputOverride",
                throughput_override_object(t),
            );
        }
        if !ru.replica_gsis.is_empty() {
            let arr = ru
                .replica_gsis
                .iter()
                .map(|g| {
                    let mut o = Json::obj();
                    o.put("IndexName", s(&g.index_name));
                    o.put(
                        "ProvisionedThroughputOverride",
                        throughput_override_object(&g.throughput_override),
                    );
                    o
                })
                .collect();
            a.put("GlobalSecondaryIndexes", Json::Array(arr));
        }
        a.put(
            "TableClassOverride",
            s(table_class_name(
                ru.table_class.unwrap_or(TableClass::Standard),
            )),
        );
        a
    };
    match r.action {
        ReplicaAction::Add => wrapper.put("Create", build(r)),
        ReplicaAction::Update => wrapper.put("Update", build(r)),
        ReplicaAction::Drop => {
            let mut a = Json::obj();
            a.put("RegionName", s(&r.region));
            wrapper.put("Delete", a);
        }
    }
    wrapper
}

fn put_names_values(o: &mut Json, names: &ExpressionAttributeNames, values: &ValueMap) {
    if !names.is_empty() {
        o.put("ExpressionAttributeNames", names_object(names));
    }
    if !values.is_empty() {
        o.put("ExpressionAttributeValues", value_map_object(values));
    }
}

fn table_only(op: &str, table: &str) -> WireRequest {
    let mut root = Json::obj();
    root.put("TableName", s(table));
    req(op, &root)
}

// ---- public serializers -------------------------------------------------------

pub fn serialize_get_item(i: &GetItemIn) -> WireRequest {
    let mut root = Json::obj();
    root.put("TableName", s(&i.table_name));
    root.put("Key", value_map_object(&i.key));
    root.put("ConsistentRead", b(i.consistent_read));
    root.put(
        "ReturnConsumedCapacity",
        s(rcc_name(i.return_consumed_capacity)),
    );
    if let Some(p) = &i.projection_expression {
        root.put("ProjectionExpression", s(p));
    }
    if !i.expression_attribute_names.is_empty() {
        root.put(
            "ExpressionAttributeNames",
            names_object(&i.expression_attribute_names),
        );
    }
    req("GetItem", &root)
}

pub fn serialize_query(i: &QueryIn) -> WireRequest {
    let mut root = Json::obj();
    root.put("TableName", s(&i.table_name));
    if let Some(x) = &i.index_name {
        root.put("IndexName", s(x));
    }
    root.put("ConsistentRead", b(i.consistent_read));
    if let Some(k) = &i.exclusive_start_key {
        root.put("ExclusiveStartKey", value_map_object(k));
    }
    root.put(
        "ReturnConsumedCapacity",
        s(rcc_name(i.return_consumed_capacity)),
    );
    if let Some(p) = &i.projection_expression {
        root.put("ProjectionExpression", s(p));
    }
    if let Some(f) = &i.filter_expression {
        root.put("FilterExpression", s(f));
    }
    root.put("KeyConditionExpression", s(&i.key_condition_expression));
    put_names_values(
        &mut root,
        &i.expression_attribute_names,
        &i.expression_attribute_values,
    );
    req("Query", &root)
}

pub fn serialize_scan(i: &ScanIn) -> WireRequest {
    let mut root = Json::obj();
    root.put("TableName", s(&i.table_name));
    if let Some(x) = &i.index_name {
        root.put("IndexName", s(x));
    }
    if let Some(k) = &i.exclusive_start_key {
        root.put("ExclusiveStartKey", value_map_object(k));
    }
    root.put(
        "ReturnConsumedCapacity",
        s(rcc_name(i.return_consumed_capacity)),
    );
    if let Some(p) = &i.projection_expression {
        root.put("ProjectionExpression", s(p));
    }
    if let Some(f) = &i.filter_expression {
        root.put("FilterExpression", s(f));
    }
    put_names_values(
        &mut root,
        &i.expression_attribute_names,
        &i.expression_attribute_values,
    );
    root.put("ConsistentRead", b(i.consistent_read));
    req("Scan", &root)
}

fn put_rv_rcc(root: &mut Json, rv: ReturnValue, rcc: ReturnConsumedCapacity) {
    if rv != ReturnValue::None {
        root.put("ReturnValues", s(rv_name(rv)));
    }
    if rcc != ReturnConsumedCapacity::None {
        root.put("ReturnConsumedCapacity", s(rcc_name(rcc)));
    }
}

pub fn serialize_put_item(i: &PutItemIn) -> WireRequest {
    let mut root = Json::obj();
    root.put("TableName", s(&i.table_name));
    root.put("Item", value_map_object(&i.item));
    put_rv_rcc(&mut root, i.return_values, i.return_consumed_capacity);
    if let Some(c) = &i.condition_expression {
        root.put("ConditionExpression", s(c));
    }
    put_names_values(
        &mut root,
        &i.expression_attribute_names,
        &i.expression_attribute_values,
    );
    req("PutItem", &root)
}

pub fn serialize_update_item(i: &UpdateItemIn) -> WireRequest {
    let mut root = Json::obj();
    root.put("TableName", s(&i.table_name));
    root.put("Key", value_map_object(&i.key));
    put_rv_rcc(&mut root, i.return_values, i.return_consumed_capacity);
    root.put("UpdateExpression", s(&i.update_expression));
    if let Some(c) = &i.condition_expression {
        root.put("ConditionExpression", s(c));
    }
    put_names_values(
        &mut root,
        &i.expression_attribute_names,
        &i.expression_attribute_values,
    );
    req("UpdateItem", &root)
}

pub fn serialize_delete_item(i: &DeleteItemIn) -> WireRequest {
    let mut root = Json::obj();
    root.put("TableName", s(&i.table_name));
    root.put("Key", value_map_object(&i.key));
    put_rv_rcc(&mut root, i.return_values, i.return_consumed_capacity);
    if let Some(c) = &i.condition_expression {
        root.put("ConditionExpression", s(c));
    }
    put_names_values(
        &mut root,
        &i.expression_attribute_names,
        &i.expression_attribute_values,
    );
    req("DeleteItem", &root)
}

pub fn serialize_create_table(i: &CreateTableIn) -> WireRequest {
    let mut root = Json::obj();
    if !i.attribute_definitions.is_empty() {
        root.put(
            "AttributeDefinitions",
            attribute_definitions_array(&i.attribute_definitions),
        );
    }
    root.put("TableName", s(&i.table_name));
    root.put("KeySchema", key_schema_array(&i.key_schema));
    if !i.lsis.is_empty() {
        root.put(
            "LocalSecondaryIndexes",
            Json::Array(i.lsis.iter().map(lsi_object).collect()),
        );
    }
    if !i.gsis.is_empty() {
        root.put(
            "GlobalSecondaryIndexes",
            Json::Array(i.gsis.iter().map(gsi_object).collect()),
        );
    }
    match &i.billing {
        Some(bm) if bm.mode == BillingMode::Provisioned => {
            root.put("BillingMode", s("PROVISIONED"));
            root.put("ProvisionedThroughput", provisioned_object(bm));
        }
        _ => root.put("BillingMode", s("PAY_PER_REQUEST")),
    }
    if let Some(st) = &i.stream {
        root.put("StreamSpecification", stream_object(st));
    }
    if let Some(sp) = &i.sse {
        root.put("SSESpecification", sse_object(sp));
    }
    if !i.tags.is_empty() {
        let arr = i
            .tags
            .iter()
            .map(|t| {
                let mut o = Json::obj();
                o.put("Key", s(&t.key));
                o.put("Value", s(&t.value));
                o
            })
            .collect();
        root.put("Tags", Json::Array(arr));
    }
    root.put("TableClass", s(table_class_name(i.table_class)));
    root.put("DeletionProtectionEnabled", b(i.deletion_protection));
    if let Some(w) = &i.warm {
        root.put("WarmThroughput", warm_object(w));
    }
    if let Some(bm) = &i.billing {
        if bm.mode == BillingMode::PayPerRequest {
            root.put("OnDemandThroughput", on_demand_object(&bm.od));
        }
    }
    if !i.vector_indexes.is_empty() {
        root.put(
            "VectorIndexes",
            Json::Array(i.vector_indexes.iter().map(vector_index_object).collect()),
        );
    }
    req("CreateTable", &root)
}

pub fn serialize_update_table(i: &UpdateTableIn) -> WireRequest {
    let mut root = Json::obj();
    if !i.attribute_definitions.is_empty() {
        root.put(
            "AttributeDefinitions",
            attribute_definitions_array(&i.attribute_definitions),
        );
    }
    root.put("TableName", s(&i.table_name));
    if let Some(bm) = &i.billing {
        if bm.mode == BillingMode::Provisioned {
            root.put("BillingMode", s("PROVISIONED"));
            root.put("ProvisionedThroughput", provisioned_object(bm));
        } else {
            root.put("BillingMode", s("PAY_PER_REQUEST"));
        }
    }
    if !i.gsi_updates.is_empty() {
        root.put(
            "GlobalSecondaryIndexUpdates",
            Json::Array(i.gsi_updates.iter().map(gsi_update_object).collect()),
        );
    }
    if let Some(st) = &i.stream {
        root.put("StreamSpecification", stream_object(st));
    }
    if let Some(sp) = &i.sse {
        root.put("SSESpecification", sse_object(sp));
    }
    if let Some(r) = &i.replica {
        root.put(
            "ReplicaUpdates",
            Json::Array(vec![replica_update_object(r)]),
        );
    }
    if let Some(tc) = i.table_class {
        root.put("TableClass", s(table_class_name(tc)));
    }
    if let Some(dp) = i.deletion_protection {
        root.put("DeletionProtectionEnabled", b(dp));
    }
    if let Some(bm) = &i.billing {
        if bm.mode == BillingMode::PayPerRequest {
            root.put("OnDemandThroughput", on_demand_object(&bm.od));
        }
    }
    if let Some(w) = &i.warm {
        root.put("WarmThroughput", warm_object(w));
    }
    if !i.vector_index_updates.is_empty() {
        root.put(
            "VectorIndexUpdates",
            Json::Array(
                i.vector_index_updates
                    .iter()
                    .map(vector_index_update_object)
                    .collect(),
            ),
        );
    }
    req("UpdateTable", &root)
}

pub fn serialize_delete_table(i: &DeleteTableIn) -> WireRequest {
    table_only("DeleteTable", &i.table_name)
}

pub fn serialize_update_time_to_live(i: &UpdateTimeToLiveIn) -> WireRequest {
    let mut root = Json::obj();
    root.put("TableName", s(&i.table_name));
    let mut spec = Json::obj();
    spec.put("Enabled", b(i.enabled));
    spec.put("AttributeName", s(&i.attribute_name));
    root.put("TimeToLiveSpecification", spec);
    req("UpdateTimeToLive", &root)
}

pub fn serialize_update_continuous_backups(i: &UpdateContinuousBackupsIn) -> WireRequest {
    let mut root = Json::obj();
    root.put("TableName", s(&i.table_name));
    let mut spec = Json::obj();
    spec.put("PointInTimeRecoveryEnabled", b(i.enabled));
    root.put("PointInTimeRecoverySpecification", spec);
    req("UpdateContinuousBackups", &root)
}

pub fn serialize_describe_table(i: &DescribeTableIn) -> WireRequest {
    table_only("DescribeTable", &i.table_name)
}
pub fn serialize_describe_time_to_live(i: &DescribeTimeToLiveIn) -> WireRequest {
    table_only("DescribeTimeToLive", &i.table_name)
}
pub fn serialize_describe_continuous_backups(i: &DescribeContinuousBackupsIn) -> WireRequest {
    table_only("DescribeContinuousBackups", &i.table_name)
}
pub fn serialize_describe_table_replica_auto_scaling(
    i: &DescribeTableReplicaAutoScalingIn,
) -> WireRequest {
    table_only("DescribeTableReplicaAutoScaling", &i.table_name)
}

pub fn serialize_describe_limits(_i: &DescribeLimitsIn) -> WireRequest {
    WireRequest {
        operation: "DescribeLimits".into(),
        body: "{}".into(),
    }
}

pub fn serialize_list_tables(exclusive_start_table_name: Option<&str>) -> WireRequest {
    match exclusive_start_table_name {
        None => WireRequest {
            operation: "ListTables".into(),
            body: "{\n}\n".into(),
        },
        Some(name) => {
            let mut root = Json::obj();
            root.put("ExclusiveStartTableName", s(name));
            req("ListTables", &root)
        }
    }
}

pub fn serialize_list_backups(i: &ListBackupsIn) -> WireRequest {
    let mut root = Json::obj();
    if let Some(t) = &i.table_name {
        root.put("TableName", s(t));
    }
    root.put("Limit", int(i.limit as i64));
    if let Some(a) = &i.exclusive_start_backup_arn {
        root.put("ExclusiveStartBackupArn", s(a));
    }
    if let Some(t) = &i.backup_type {
        root.put("BackupType", s(t));
    }
    req("ListBackups", &root)
}

pub fn serialize_create_backup(i: &CreateBackupIn) -> WireRequest {
    let mut root = Json::obj();
    root.put("TableName", s(&i.table_name));
    root.put("BackupName", s(&i.backup_name));
    req("CreateBackup", &root)
}

pub fn serialize_delete_backup(i: &DeleteBackupIn) -> WireRequest {
    let mut root = Json::obj();
    root.put("BackupArn", s(&i.backup_arn));
    req("DeleteBackup", &root)
}

pub fn serialize_describe_backup(i: &DescribeBackupIn) -> WireRequest {
    let mut root = Json::obj();
    root.put("BackupArn", s(&i.backup_arn));
    req("DescribeBackup", &root)
}

pub fn serialize_restore_from_backup(i: &RestoreFromBackupIn) -> WireRequest {
    let mut root = Json::obj();
    root.put("TargetTableName", s(&i.target_table_name));
    root.put("BackupArn", s(&i.backup_arn));
    req("RestoreTableFromBackup", &root)
}

pub fn serialize_restore_to_pitr(i: &RestoreToPitrIn) -> WireRequest {
    let mut root = Json::obj();
    root.put("SourceTableName", s(&i.source_table_name));
    root.put("TargetTableName", s(&i.target_table_name));
    let epoch = iso8601_to_epoch(&i.restore_datetime).unwrap_or(0.0);
    root.put("RestoreDateTime", Json::Number(cjson_number_double(epoch)));
    req("RestoreTableToPointInTime", &root)
}

pub fn serialize_transact_get_items(i: &TransactGetItemsIn) -> WireRequest {
    let items = i
        .items
        .iter()
        .map(|g| {
            let mut get = Json::obj();
            get.put("Key", value_map_object(&g.key));
            get.put("TableName", s(&g.table_name));
            if let Some(p) = &g.projection_expression {
                get.put("ProjectionExpression", s(p));
            }
            if !g.expression_attribute_names.is_empty() {
                get.put(
                    "ExpressionAttributeNames",
                    names_object(&g.expression_attribute_names),
                );
            }
            let mut item = Json::obj();
            item.put("Get", get);
            item
        })
        .collect();
    let mut root = Json::obj();
    root.put("TransactItems", Json::Array(items));
    req("TransactGetItems", &root)
}

pub fn serialize_transact_write_items(i: &TransactWriteItemsIn) -> WireRequest {
    let items = i
        .items
        .iter()
        .map(|w| {
            let mut item = Json::obj();
            match w {
                TransactWriteItem::Put(p) => {
                    let mut o = Json::obj();
                    o.put("Item", value_map_object(&p.item));
                    o.put("TableName", s(&p.table_name));
                    if let Some(c) = &p.condition_expression {
                        o.put("ConditionExpression", s(c));
                    }
                    put_names_values(
                        &mut o,
                        &p.expression_attribute_names,
                        &p.expression_attribute_values,
                    );
                    item.put("Put", o);
                }
                TransactWriteItem::Update(u) => {
                    let mut o = Json::obj();
                    o.put("Key", value_map_object(&u.key));
                    o.put("UpdateExpression", s(&u.update_expression));
                    o.put("TableName", s(&u.table_name));
                    if let Some(c) = &u.condition_expression {
                        o.put("ConditionExpression", s(c));
                    }
                    put_names_values(
                        &mut o,
                        &u.expression_attribute_names,
                        &u.expression_attribute_values,
                    );
                    item.put("Update", o);
                }
                TransactWriteItem::Delete(d) => {
                    let mut o = Json::obj();
                    o.put("Key", value_map_object(&d.key));
                    o.put("TableName", s(&d.table_name));
                    if let Some(c) = &d.condition_expression {
                        o.put("ConditionExpression", s(c));
                    }
                    put_names_values(
                        &mut o,
                        &d.expression_attribute_names,
                        &d.expression_attribute_values,
                    );
                    item.put("Delete", o);
                }
                TransactWriteItem::ConditionCheck(c) => {
                    let mut o = Json::obj();
                    o.put("Key", value_map_object(&c.key));
                    o.put("TableName", s(&c.table_name));
                    o.put("ConditionExpression", s(&c.condition_expression));
                    put_names_values(
                        &mut o,
                        &c.expression_attribute_names,
                        &c.expression_attribute_values,
                    );
                    item.put("ConditionCheck", o);
                }
            }
            item
        })
        .collect();
    let mut root = Json::obj();
    root.put("TransactItems", Json::Array(items));
    req("TransactWriteItems", &root)
}

pub fn serialize_search_vectors(i: &SearchVectorsIn) -> WireRequest {
    let mut root = Json::obj();
    root.put("TableName", s(&i.table_name));
    root.put("IndexName", s(&i.index_name));
    root.put(
        "ReturnConsumedCapacity",
        s(rcc_name(i.return_consumed_capacity)),
    );
    put_names_values(
        &mut root,
        &i.expression_attribute_names,
        &i.expression_attribute_values,
    );
    if let Some(p) = &i.projection_expression {
        root.put("ProjectionExpression", s(p));
    }
    root.put("SearchVector", value_list_array(&i.search_vector));
    if let Some(c) = &i.search_condition_expression {
        root.put("SearchConditionExpression", s(c));
    }
    root.put("TopK", int(i.topk as i64));
    req("SearchVectors", &root)
}

/// Serialize any EXPLAIN-able request.
pub fn serialize(request: &ExplainRequest) -> WireRequest {
    match request {
        ExplainRequest::GetItem(i) => serialize_get_item(i),
        ExplainRequest::Query(i) => serialize_query(i),
        ExplainRequest::Scan(i) => serialize_scan(i),
        ExplainRequest::PutItem(i) => serialize_put_item(i),
        ExplainRequest::CreateTable(i) => serialize_create_table(i),
        ExplainRequest::UpdateTable(i) => serialize_update_table(i),
        ExplainRequest::UpdateTimeToLive(i) => serialize_update_time_to_live(i),
        ExplainRequest::UpdateContinuousBackups(i) => serialize_update_continuous_backups(i),
        ExplainRequest::DeleteTable(i) => serialize_delete_table(i),
        ExplainRequest::DescribeTable(i) => serialize_describe_table(i),
        ExplainRequest::DescribeTimeToLive(i) => serialize_describe_time_to_live(i),
        ExplainRequest::DescribeContinuousBackups(i) => serialize_describe_continuous_backups(i),
        ExplainRequest::DescribeTableReplicaAutoScaling(i) => {
            serialize_describe_table_replica_auto_scaling(i)
        }
        ExplainRequest::UpdateItem(i) => serialize_update_item(i),
        ExplainRequest::DeleteItem(i) => serialize_delete_item(i),
        ExplainRequest::TransactGetItems(i) => serialize_transact_get_items(i),
        ExplainRequest::TransactWriteItems(i) => serialize_transact_write_items(i),
        ExplainRequest::CreateBackup(i) => serialize_create_backup(i),
        ExplainRequest::DeleteBackup(i) => serialize_delete_backup(i),
        ExplainRequest::DescribeBackup(i) => serialize_describe_backup(i),
        ExplainRequest::RestoreFromBackup(i) => serialize_restore_from_backup(i),
        ExplainRequest::RestoreToPitr(i) => serialize_restore_to_pitr(i),
        ExplainRequest::SearchVectors(i) => serialize_search_vectors(i),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The renderer's pretty transform (tab -> 3 spaces), for golden comparison.
    fn pretty(body: &str) -> String {
        body.replace('\t', "   ")
    }

    #[test]
    fn get_item_matches_golden_basic_test_6() {
        let i = GetItemIn {
            table_name: "ddbsh_test_table_1".into(),
            key: vec![("a".into(), Value::N("3".into()))],
            ..Default::default()
        };
        let out = serialize_get_item(&i);
        assert_eq!(out.operation, "GetItem");
        let expected = "{\n   \"TableName\":   \"ddbsh_test_table_1\",\n   \"Key\":   {\n      \"a\":   {\n         \"N\":   \"3\"\n      }\n   },\n   \"ConsistentRead\":   false,\n   \"ReturnConsumedCapacity\":   \"NONE\"\n}";
        assert_eq!(pretty(&out.body), expected);
    }

    #[test]
    fn scan_matches_golden_basic_test_2() {
        let i = ScanIn {
            table_name: "ddbsh_test_table_1".into(),
            ..Default::default()
        };
        let expected = "{\n   \"TableName\":   \"ddbsh_test_table_1\",\n   \"ReturnConsumedCapacity\":   \"NONE\",\n   \"ConsistentRead\":   false\n}";
        assert_eq!(pretty(&serialize_scan(&i).body), expected);
    }

    #[test]
    fn maps_sorted_and_arrays_inline() {
        let i = PutItemIn {
            table_name: "t".into(),
            item: vec![
                (
                    "z".into(),
                    Value::L(vec![Value::N("1".into()), Value::S("x".into())]),
                ),
                (
                    "a".into(),
                    Value::M(vec![
                        ("q".into(), Value::Bool(true)),
                        ("b".into(), Value::Null),
                    ]),
                ),
                ("ss".into(), Value::SS(vec!["p".into(), "q\"".into()])),
            ],
            ..Default::default()
        };
        let body = serialize_put_item(&i).body;
        assert_eq!(
            body,
            "{\n\t\"TableName\":\t\"t\",\n\t\"Item\":\t{\n\t\t\"a\":\t{\n\t\t\t\"M\":\t{\n\t\t\t\t\"b\":\t{\n\t\t\t\t\t\"NULL\":\ttrue\n\t\t\t\t},\n\t\t\t\t\"q\":\t{\n\t\t\t\t\t\"BOOL\":\ttrue\n\t\t\t\t}\n\t\t\t}\n\t\t},\n\t\t\"ss\":\t{\n\t\t\t\"SS\":\t[\"p\", \"q\\\"\"]\n\t\t},\n\t\t\"z\":\t{\n\t\t\t\"L\":\t[{\n\t\t\t\t\t\"N\":\t\"1\"\n\t\t\t\t}, {\n\t\t\t\t\t\"S\":\t\"x\"\n\t\t\t\t}]\n\t\t}\n\t}\n}"
        );
    }

    #[test]
    fn list_tables_and_limits_literals() {
        assert_eq!(serialize_list_tables(None).body, "{\n}\n");
        assert_eq!(serialize_describe_limits(&DescribeLimitsIn {}).body, "{}");
        assert_eq!(
            serialize_list_tables(Some("x")).body,
            "{\n\t\"ExclusiveStartTableName\":\t\"x\"\n}"
        );
    }

    #[test]
    fn create_table_default_billing() {
        let i = CreateTableIn {
            table_name: "t".into(),
            attribute_definitions: vec![AttributeDef {
                name: "a".into(),
                ty: ScalarType::N,
            }],
            key_schema: KeySchema {
                hash: vec!["a".into()],
                range: vec![],
            },
            ..Default::default()
        };
        let expected = "{\n\t\"AttributeDefinitions\":\t[{\n\t\t\t\"AttributeName\":\t\"a\",\n\t\t\t\"AttributeType\":\t\"N\"\n\t\t}],\n\t\"TableName\":\t\"t\",\n\t\"KeySchema\":\t[{\n\t\t\t\"AttributeName\":\t\"a\",\n\t\t\t\"KeyType\":\t\"HASH\"\n\t\t}],\n\t\"BillingMode\":\t\"PAY_PER_REQUEST\",\n\t\"TableClass\":\t\"STANDARD\",\n\t\"DeletionProtectionEnabled\":\tfalse\n}";
        assert_eq!(serialize_create_table(&i).body, expected);
    }

    #[test]
    fn number_formatting() {
        assert_eq!(cjson_number_double(1700000000.123), "1700000000.123");
        assert_eq!(cjson_number_double(0.0), "0");
        assert_eq!(cjson_number_double(1.0), "1");
        assert_eq!(cjson_number_double(0.1), "0.1");
        assert_eq!(cjson_number_double(1e20), "1e+20");
        assert_eq!(cjson_number_double(1.0 / 3.0), "0.33333333333333331");
        assert_eq!(format_g(0.0001, 15), "0.0001");
        assert_eq!(format_g(0.00001, 15), "1e-05");
    }

    #[test]
    fn string_escapes() {
        let i = DescribeTableIn {
            table_name: "a\"\\\u{1}\n".into(),
        };
        assert_eq!(
            serialize_describe_table(&i).body,
            "{\n\t\"TableName\":\t\"a\\\"\\\\\\u0001\\n\"\n}"
        );
    }

    #[test]
    fn restore_to_pitr_epoch() {
        let i = RestoreToPitrIn {
            source_table_name: "s".into(),
            target_table_name: "t".into(),
            restore_datetime: "2023-11-14T22:13:20.500Z".into(),
        };
        assert!(serialize_restore_to_pitr(&i)
            .body
            .contains("\"RestoreDateTime\":\t1700000000.5\n"));
    }
}
