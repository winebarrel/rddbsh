// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! The SDK-free DynamoDB client seam (port of include/ddbsh/iddbclient.hpp).
//! Executors talk only to `DdbClient`; the wire client implements it.

use crate::error::{fail, Errc, Result};
use crate::table_schema::*;
use crate::value::{ValueList, ValueMap};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ItemRow {
    pub attributes: ValueMap,
}

pub type ExpressionAttributeNames = Vec<(String, String)>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReturnConsumedCapacity {
    #[default]
    None,
    Total,
    Indexes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReturnValue {
    #[default]
    None,
    AllOld,
    AllNew,
    UpdatedOld,
    UpdatedNew,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ConsumedCapacity {
    pub table_name: Option<String>,
    pub capacity_units: Option<f64>,
    pub read_capacity_units: Option<f64>,
    pub write_capacity_units: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct GetItemIn {
    pub table_name: String,
    pub key: ValueMap,
    pub projection_expression: Option<String>,
    pub expression_attribute_names: ExpressionAttributeNames,
    pub consistent_read: bool,
    pub return_consumed_capacity: ReturnConsumedCapacity,
}
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GetItemOut {
    pub item: Option<ItemRow>,
    pub consumed_capacity: Option<ConsumedCapacity>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct QueryIn {
    pub table_name: String,
    pub index_name: Option<String>,
    pub key_condition_expression: String,
    pub filter_expression: Option<String>,
    pub projection_expression: Option<String>,
    pub expression_attribute_names: ExpressionAttributeNames,
    pub expression_attribute_values: ValueMap,
    pub consistent_read: bool,
    pub return_consumed_capacity: ReturnConsumedCapacity,
    pub exclusive_start_key: Option<ValueMap>,
}
#[derive(Debug, Clone, PartialEq, Default)]
pub struct QueryOut {
    pub items: Vec<ItemRow>,
    pub last_evaluated_key: Option<ValueMap>,
    pub consumed_capacity: Option<ConsumedCapacity>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScanIn {
    pub table_name: String,
    pub index_name: Option<String>,
    pub filter_expression: Option<String>,
    pub projection_expression: Option<String>,
    pub expression_attribute_names: ExpressionAttributeNames,
    pub expression_attribute_values: ValueMap,
    pub consistent_read: bool,
    pub return_consumed_capacity: ReturnConsumedCapacity,
    pub exclusive_start_key: Option<ValueMap>,
}
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScanOut {
    pub items: Vec<ItemRow>,
    pub last_evaluated_key: Option<ValueMap>,
    pub consumed_capacity: Option<ConsumedCapacity>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PutItemIn {
    pub table_name: String,
    pub item: ValueMap,
    pub condition_expression: Option<String>,
    pub expression_attribute_names: ExpressionAttributeNames,
    pub expression_attribute_values: ValueMap,
    pub return_values: ReturnValue,
    pub return_consumed_capacity: ReturnConsumedCapacity,
}
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PutItemOut {
    pub attributes: Option<ItemRow>,
    pub consumed_capacity: Option<ConsumedCapacity>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateItemIn {
    pub table_name: String,
    pub key: ValueMap,
    pub update_expression: String,
    pub condition_expression: Option<String>,
    pub expression_attribute_names: ExpressionAttributeNames,
    pub expression_attribute_values: ValueMap,
    pub return_values: ReturnValue,
    pub return_consumed_capacity: ReturnConsumedCapacity,
}
#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateItemOut {
    pub attributes: Option<ItemRow>,
    pub consumed_capacity: Option<ConsumedCapacity>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeleteItemIn {
    pub table_name: String,
    pub key: ValueMap,
    pub condition_expression: Option<String>,
    pub expression_attribute_names: ExpressionAttributeNames,
    pub expression_attribute_values: ValueMap,
    pub return_values: ReturnValue,
    pub return_consumed_capacity: ReturnConsumedCapacity,
}
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeleteItemOut {
    pub attributes: Option<ItemRow>,
    pub consumed_capacity: Option<ConsumedCapacity>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeleteTableIn {
    pub table_name: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CreateTableIn {
    pub table_name: String,
    pub attribute_definitions: Vec<AttributeDef>,
    pub key_schema: KeySchema,
    pub billing: Option<BillingModeAndThroughput>,
    pub warm: Option<WarmThroughput>,
    pub gsis: Vec<Gsi>,
    pub lsis: Vec<Lsi>,
    pub vector_indexes: Vec<VectorIndexDef>,
    pub stream: Option<StreamSpec>,
    pub sse: Option<SseSpec>,
    pub table_class: TableClass,
    pub deletion_protection: bool,
    pub tags: Vec<Tag>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateTableIn {
    pub table_name: String,
    pub billing: Option<BillingModeAndThroughput>,
    pub warm: Option<WarmThroughput>,
    pub sse: Option<SseSpec>,
    pub stream: Option<StreamSpec>,
    pub table_class: Option<TableClass>,
    pub deletion_protection: Option<bool>,
    pub attribute_definitions: Vec<AttributeDef>,
    pub gsi_updates: Vec<GsiUpdate>,
    pub vector_index_updates: Vec<VectorIndexUpdateDef>,
    pub replica: Option<ReplicaUpdate>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateTimeToLiveIn {
    pub table_name: String,
    pub enabled: bool,
    pub attribute_name: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateContinuousBackupsIn {
    pub table_name: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CreateBackupIn {
    pub table_name: String,
    pub backup_name: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeleteBackupIn {
    pub backup_arn: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct BackupKeySchemaElement {
    pub key_type: String,
    pub attribute_name: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DescribeBackupIn {
    pub backup_arn: String,
}
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DescribeBackupOut {
    pub source_table_name: String,
    pub source_table_id: String,
    pub source_table_arn: String,
    pub table_size_bytes: i64,
    pub item_count: i64,
    pub key_schema: Vec<BackupKeySchemaElement>,
    pub table_creation_datetime: String,
    pub billing_mode: Option<String>,
    pub provisioned: Option<(i64, i64)>,
    pub backup_name: String,
    pub backup_arn: String,
    pub backup_size_bytes: i64,
    pub backup_status: String,
    pub backup_type: String,
    pub backup_creation_datetime: String,
    pub backup_expiry_datetime: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RestoreFromBackupIn {
    pub target_table_name: String,
    pub backup_arn: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RestoreToPitrIn {
    pub source_table_name: String,
    pub target_table_name: String,
    pub restore_datetime: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DescribeLimitsIn {}
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DescribeLimitsOut {
    pub account_max_rcu: i64,
    pub account_max_wcu: i64,
    pub table_max_rcu: i64,
    pub table_max_wcu: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListBackupsIn {
    pub table_name: Option<String>,
    pub backup_type: Option<String>,
    pub limit: i32,
    pub exclusive_start_backup_arn: Option<String>,
}
impl Default for ListBackupsIn {
    fn default() -> Self {
        ListBackupsIn {
            table_name: None,
            backup_type: None,
            limit: 100,
            exclusive_start_backup_arn: None,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BackupSummaryRow {
    pub table_name: String,
    pub backup_name: String,
    pub backup_status: String,
    pub backup_arn: String,
    pub creation_datetime: String,
    pub expiry_datetime: String,
}
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ListBackupsOut {
    pub summaries: Vec<BackupSummaryRow>,
    pub last_evaluated_backup_arn: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ListTablesIn {}
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ListTablesOut {
    pub table_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TableKeySchema {
    pub hash_keys: Vec<String>,
    pub range_keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct WarmThroughputDescription {
    pub read_units_per_second: i64,
    pub write_units_per_second: i64,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SecondaryIndexSchema {
    pub index_name: String,
    pub key_schema: TableKeySchema,
    pub projection: Projection,
    pub projection_set: bool,
    pub index_status: String,
    pub backfilling: bool,
    pub provisioned_read_capacity_units: i64,
    pub provisioned_write_capacity_units: i64,
    pub on_demand_throughput_set: bool,
    pub on_demand_max_read_request_units: i64,
    pub on_demand_max_write_request_units: i64,
    pub warm_throughput: Option<WarmThroughputDescription>,
}
impl Default for SecondaryIndexSchema {
    fn default() -> Self {
        SecondaryIndexSchema {
            index_name: String::new(),
            key_schema: TableKeySchema::default(),
            projection: Projection::default(),
            projection_set: false,
            index_status: String::new(),
            backfilling: false,
            provisioned_read_capacity_units: 0,
            provisioned_write_capacity_units: 0,
            on_demand_throughput_set: false,
            on_demand_max_read_request_units: -1,
            on_demand_max_write_request_units: -1,
            warm_throughput: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct VectorIndexSchema {
    pub index_name: String,
    pub vector_attr: String,
    pub dimensions: i64,
    pub distance: VectorDistance,
    pub search_schema: Vec<SearchSchemaElem>,
    pub projection: Projection,
    pub projection_set: bool,
    pub index_status: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DescribeTableIn {
    pub table_name: String,
}
#[derive(Debug, Clone, PartialEq)]
pub struct DescribeTableOut {
    pub table_name: String,
    pub key_schema: TableKeySchema,
    pub local_secondary_indexes: Vec<SecondaryIndexSchema>,
    pub global_secondary_indexes: Vec<SecondaryIndexSchema>,
    pub vector_indexes: Vec<VectorIndexSchema>,
    pub table_status: String,
    pub table_id: String,
    pub table_arn: String,
    pub billing_mode_summary: Option<String>,
    pub provisioned_throughput_set: bool,
    pub table_class_summary: Option<String>,
    pub attribute_definitions: Vec<AttributeDef>,
    pub creation_date_time: String,
    pub table_size_bytes: i64,
    pub item_count: i64,
    pub deletion_protection_enabled: bool,
    pub provisioned_read_capacity_units: i64,
    pub provisioned_write_capacity_units: i64,
    pub on_demand_throughput_set: bool,
    pub on_demand_max_read_request_units: i64,
    pub on_demand_max_write_request_units: i64,
    pub warm_throughput: Option<WarmThroughputDescription>,
    pub stream_view_type: Option<String>,
    pub stream_specification_set: bool,
    pub sse_status: Option<String>,
}
impl Default for DescribeTableOut {
    fn default() -> Self {
        DescribeTableOut {
            table_name: String::new(),
            key_schema: TableKeySchema::default(),
            local_secondary_indexes: vec![],
            global_secondary_indexes: vec![],
            vector_indexes: vec![],
            table_status: String::new(),
            table_id: String::new(),
            table_arn: String::new(),
            billing_mode_summary: None,
            provisioned_throughput_set: false,
            table_class_summary: None,
            attribute_definitions: vec![],
            creation_date_time: String::new(),
            table_size_bytes: 0,
            item_count: 0,
            deletion_protection_enabled: false,
            provisioned_read_capacity_units: 0,
            provisioned_write_capacity_units: 0,
            on_demand_throughput_set: false,
            on_demand_max_read_request_units: -1,
            on_demand_max_write_request_units: -1,
            warm_throughput: None,
            stream_view_type: None,
            stream_specification_set: false,
            sse_status: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DescribeTimeToLiveIn {
    pub table_name: String,
}
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TimeToLiveOut {
    pub status: String,
    pub attribute_name: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DescribeContinuousBackupsIn {
    pub table_name: String,
}
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ContinuousBackupsOut {
    pub pitr_status: String,
    pub earliest_restorable_date_time: String,
    pub latest_restorable_date_time: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ReplicaAutoScalingDescription {
    pub region_name: String,
    pub replica_status: String,
}
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DescribeTableReplicaAutoScalingIn {
    pub table_name: String,
}
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TableReplicaAutoScalingOut {
    pub replicas: Vec<ReplicaAutoScalingDescription>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TransactGet {
    pub table_name: String,
    pub key: ValueMap,
    pub projection_expression: Option<String>,
    pub expression_attribute_names: ExpressionAttributeNames,
}
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TransactGetItemsIn {
    pub items: Vec<TransactGet>,
}
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TransactGetItemsOut {
    pub responses: Vec<Option<ItemRow>>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TransactPut {
    pub table_name: String,
    pub item: ValueMap,
    pub condition_expression: Option<String>,
    pub expression_attribute_names: ExpressionAttributeNames,
    pub expression_attribute_values: ValueMap,
}
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TransactUpdate {
    pub table_name: String,
    pub key: ValueMap,
    pub update_expression: String,
    pub condition_expression: Option<String>,
    pub expression_attribute_names: ExpressionAttributeNames,
    pub expression_attribute_values: ValueMap,
}
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TransactDelete {
    pub table_name: String,
    pub key: ValueMap,
    pub condition_expression: Option<String>,
    pub expression_attribute_names: ExpressionAttributeNames,
    pub expression_attribute_values: ValueMap,
}
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TransactConditionCheck {
    pub table_name: String,
    pub key: ValueMap,
    pub condition_expression: String,
    pub expression_attribute_names: ExpressionAttributeNames,
    pub expression_attribute_values: ValueMap,
}
#[derive(Debug, Clone, PartialEq)]
pub enum TransactWriteItem {
    Put(TransactPut),
    Update(TransactUpdate),
    Delete(TransactDelete),
    ConditionCheck(TransactConditionCheck),
}
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TransactWriteItemsIn {
    pub items: Vec<TransactWriteItem>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct VectorConsumedCapacity {
    pub vector_search_request_bytes: Option<f64>,
    pub vector_write_request_bytes: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SearchVectorsIn {
    pub table_name: String,
    pub index_name: String,
    pub search_vector: ValueList,
    pub topk: i32,
    pub search_condition_expression: Option<String>,
    pub projection_expression: Option<String>,
    pub expression_attribute_names: ExpressionAttributeNames,
    pub expression_attribute_values: ValueMap,
    pub return_consumed_capacity: ReturnConsumedCapacity,
}
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SearchVectorRow {
    pub item: ItemRow,
    pub score: f64,
}
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SearchVectorsOut {
    pub results: Vec<SearchVectorRow>,
    pub vector_capacity: Option<VectorConsumedCapacity>,
}

/// Every request EXPLAIN can serialize.
#[derive(Debug, Clone, PartialEq)]
pub enum ExplainRequest {
    GetItem(GetItemIn),
    Query(QueryIn),
    Scan(ScanIn),
    PutItem(PutItemIn),
    CreateTable(CreateTableIn),
    UpdateTable(UpdateTableIn),
    UpdateTimeToLive(UpdateTimeToLiveIn),
    UpdateContinuousBackups(UpdateContinuousBackupsIn),
    DeleteTable(DeleteTableIn),
    DescribeTable(DescribeTableIn),
    DescribeTimeToLive(DescribeTimeToLiveIn),
    DescribeContinuousBackups(DescribeContinuousBackupsIn),
    DescribeTableReplicaAutoScaling(DescribeTableReplicaAutoScalingIn),
    UpdateItem(UpdateItemIn),
    DeleteItem(DeleteItemIn),
    TransactGetItems(TransactGetItemsIn),
    TransactWriteItems(TransactWriteItemsIn),
    CreateBackup(CreateBackupIn),
    DeleteBackup(DeleteBackupIn),
    DescribeBackup(DescribeBackupIn),
    RestoreFromBackup(RestoreFromBackupIn),
    RestoreToPitr(RestoreToPitrIn),
    SearchVectors(SearchVectorsIn),
}

/// The client seam. Every method returns `Result`; nothing panics on
/// user/service/data paths.
pub trait DdbClient {
    fn connect(&mut self, _region: &str, _endpoint: &str) -> Result<()> {
        Ok(())
    }
    fn get_item(&mut self, input: &GetItemIn) -> Result<GetItemOut>;
    fn query(&mut self, input: &QueryIn) -> Result<QueryOut>;
    fn scan(&mut self, input: &ScanIn) -> Result<ScanOut>;
    fn search_vectors(&mut self, _input: &SearchVectorsIn) -> Result<SearchVectorsOut> {
        fail(
            Errc::ServiceError,
            "SearchVectors is not supported by this client",
        )
    }
    fn put_item(&mut self, input: &PutItemIn) -> Result<PutItemOut>;
    fn create_table(&mut self, input: &CreateTableIn) -> Result<()>;
    fn update_table(&mut self, input: &UpdateTableIn) -> Result<()>;
    fn update_time_to_live(&mut self, input: &UpdateTimeToLiveIn) -> Result<()>;
    fn update_continuous_backups(&mut self, input: &UpdateContinuousBackupsIn) -> Result<()>;
    fn create_backup(&mut self, input: &CreateBackupIn) -> Result<()>;
    fn delete_backup(&mut self, input: &DeleteBackupIn) -> Result<()>;
    fn describe_backup(&mut self, input: &DescribeBackupIn) -> Result<DescribeBackupOut>;
    fn restore_table_from_backup(&mut self, input: &RestoreFromBackupIn) -> Result<()>;
    fn restore_table_to_point_in_time(&mut self, input: &RestoreToPitrIn) -> Result<()>;
    fn describe_limits(&mut self, input: &DescribeLimitsIn) -> Result<DescribeLimitsOut>;
    fn list_backups(&mut self, input: &ListBackupsIn) -> Result<ListBackupsOut>;
    fn delete_table(&mut self, input: &DeleteTableIn) -> Result<()>;
    fn list_tables(&mut self, input: &ListTablesIn) -> Result<ListTablesOut>;
    fn describe_table(&mut self, input: &DescribeTableIn) -> Result<DescribeTableOut>;
    fn update_item(&mut self, input: &UpdateItemIn) -> Result<UpdateItemOut>;
    fn delete_item(&mut self, input: &DeleteItemIn) -> Result<DeleteItemOut>;
    fn transact_get_items(&mut self, input: &TransactGetItemsIn) -> Result<TransactGetItemsOut>;
    fn transact_write_items(&mut self, input: &TransactWriteItemsIn) -> Result<()>;
    fn describe_time_to_live(&mut self, input: &DescribeTimeToLiveIn) -> Result<TimeToLiveOut>;
    fn describe_continuous_backups(
        &mut self,
        input: &DescribeContinuousBackupsIn,
    ) -> Result<ContinuousBackupsOut>;
    fn describe_table_replica_auto_scaling(
        &mut self,
        input: &DescribeTableReplicaAutoScalingIn,
    ) -> Result<TableReplicaAutoScalingOut>;
    fn wait_for_table_active(&mut self, table_name: &str) -> Result<()>;
    fn wait_for_table_gone(&mut self, table_name: &str) -> Result<()>;
    /// Serialize the exact request the op would send (tab-indented JSON body).
    fn explain_request(&mut self, request: &ExplainRequest) -> Result<String>;
    fn connection_descriptor(&self) -> String {
        "direct client".to_string()
    }
}
