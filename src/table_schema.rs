// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! Table-schema value types shared by the AST and the client seam
//! (port of include/ddbsh/table_schema.hpp).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScalarType {
    N,
    #[default]
    S,
    B,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AttributeDef {
    pub name: String,
    pub ty: ScalarType,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct KeySchema {
    /// composite partition key, size 1..4
    pub hash: Vec<String>,
    /// composite sort key, size 0..4
    pub range: Vec<String>,
}

pub fn add_key_role(ks: &mut KeySchema, is_hash: bool, attrs: Vec<String>) {
    if is_hash {
        ks.hash.extend(attrs);
    } else {
        ks.range.extend(attrs);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BillingMode {
    #[default]
    PayPerRequest,
    Provisioned,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct OnDemandThroughput {
    pub max_read_request_units: Option<i64>,
    pub max_write_request_units: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct BillingModeAndThroughput {
    pub mode: BillingMode,
    pub rcu: i64,
    pub wcu: i64,
    pub od: OnDemandThroughput,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct WarmThroughput {
    pub read_units_per_second: i64,
    pub write_units_per_second: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProjectionType {
    #[default]
    All,
    KeysOnly,
    Include,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Projection {
    pub ty: ProjectionType,
    /// only for `Include`
    pub non_key_attributes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Gsi {
    pub name: String,
    pub key_schema: KeySchema,
    pub projection: Projection,
    pub billing: Option<BillingModeAndThroughput>,
    pub warm: Option<WarmThroughput>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GsiAction {
    Create,
    #[default]
    Update,
    Drop,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct GsiUpdate {
    pub action: GsiAction,
    pub index_name: String,
    pub key_schema: Option<KeySchema>,
    pub projection: Option<Projection>,
    pub billing: Option<BillingModeAndThroughput>,
    pub warm: Option<WarmThroughput>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Lsi {
    pub name: String,
    pub key_schema: KeySchema,
    pub projection: Projection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VectorDistance {
    #[default]
    Cosine,
    Euclidean,
    DotProduct,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchSchemaKind {
    Hash,
    #[default]
    InlineFilter,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SearchSchemaElem {
    pub attr: String,
    pub kind: SearchSchemaKind,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct VectorIndexDef {
    pub index_name: String,
    pub vector_attr: String,
    pub dimensions: i64,
    pub distance: VectorDistance,
    pub search_schema: Vec<SearchSchemaElem>,
    pub projection: Projection,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct VectorIndexUpdateDef {
    pub create: Option<VectorIndexDef>,
    pub drop: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StreamViewType {
    #[default]
    KeysOnly,
    NewImage,
    OldImage,
    NewAndOldImages,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct StreamSpec {
    pub enabled: bool,
    pub view_type: StreamViewType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SseType {
    Kms,
    Aes256,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SseSpec {
    pub enabled: bool,
    pub kms_master_key_id: Option<String>,
    pub sse_type: Option<SseType>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TableClass {
    #[default]
    Standard,
    StandardInfrequentAccess,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Tag {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProvisionedThroughputOverride {
    pub read_capacity_units: i64,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ReplicaGsi {
    pub index_name: String,
    pub throughput_override: ProvisionedThroughputOverride,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReplicaAction {
    #[default]
    Add,
    Drop,
    Update,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ReplicaUpdate {
    pub action: ReplicaAction,
    pub region: String,
    pub table_class: Option<TableClass>,
    pub throughput_override: Option<ProvisionedThroughputOverride>,
    pub replica_gsis: Vec<ReplicaGsi>,
}
