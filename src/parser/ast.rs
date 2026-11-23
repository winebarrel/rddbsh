// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! The parsed command model (port of src/parser/ast.hpp).

pub use crate::client::{ReturnConsumedCapacity, ReturnValue};
use crate::expressions::Where;
use crate::statement::StatementKind;
pub use crate::table_schema::*;
use crate::value::{Value, ValueList};

/// Strip one pair of surrounding double quotes.
pub fn unquote(s: &str) -> String {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// strtoll-like: leading integer prefix, 0 on failure.
pub fn parse_ll(s: &str) -> i64 {
    let t = s.trim_start();
    let mut end = 0;
    for (i, c) in t.char_indices() {
        if c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+')) {
            end = i + c.len_utf8();
        } else {
            break;
        }
    }
    t[..end].parse().unwrap_or(0)
}

/// strtod-like: parse as f64, 0 on failure.
pub fn parse_double(s: &str) -> f64 {
    s.trim().parse().unwrap_or(0.0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HelpTopic {
    #[default]
    General,
    Explain,
    Begin,
    Commit,
    Rollback,
    Connect,
    Drop,
    DropTable,
    Describe,
    Show,
    ShowTables,
    Create,
    CreateTable,
    AlterTable,
    ShowLimits,
    ShowBackups,
    DescribeBackup,
    DropBackup,
    Backup,
    Restore,
    SelectStatement,
    Insert,
    Replace,
    Update,
    Upsert,
    Del,
    ShowVersion,
    ShowCreateTable,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct HelpCmd {
    pub topic: HelpTopic,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ConnectCmd {
    pub region: String,
    /// empty when no WITH ENDPOINT clause
    pub endpoint: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ShowTablesCmd {
    pub brief: bool,
    /// empty when no LIKE clause
    pub like: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DescribeCmd {
    pub table: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DropTableCmd {
    pub table: String,
    pub if_exists: bool,
    pub nowait: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct KeyRole {
    pub is_hash: bool,
    pub attrs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CreateTableCmd {
    pub table_name: String,
    pub if_not_exists: bool,
    pub nowait: bool,
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

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RateLimit {
    pub rcu: f64,
    pub wcu: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct InsertCmd {
    pub table_name: String,
    pub columns: Vec<String>,
    pub rows: Vec<ValueList>,
    /// true = INSERT, false = REPLACE
    pub insert: bool,
    pub ratelimit: Option<RateLimit>,
    pub return_value: ReturnValue,
}
impl Default for InsertCmd {
    fn default() -> Self {
        InsertCmd {
            table_name: String::new(),
            columns: vec![],
            rows: vec![],
            insert: true,
            ratelimit: None,
            return_value: ReturnValue::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExistsMode {
    #[default]
    None,
    Exists,
    NotExists,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TableReference {
    pub table: String,
    pub index: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SelectCmd {
    pub consistent: bool,
    /// empty => `*`
    pub projection: Vec<String>,
    pub table: String,
    pub index: Option<String>,
    pub where_: Option<Where>,
    pub rcc: ReturnConsumedCapacity,
    pub ratelimit: Option<RateLimit>,
    pub exists_mode: ExistsMode,
    pub search_vector: Option<Value>,
    pub topk: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateSetElement {
    pub lhs: String,
    /// "" | "+" | "-"
    pub op: String,
    pub rhs_value: Option<Value>,
    pub rhs_attribute: String,
    pub if_not_exists_lhs: String,
    pub if_not_exists_value: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mutation {
    #[default]
    Set,
    Remove,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct UpdateCmd {
    pub table: String,
    pub index: Option<String>,
    pub mutation: Mutation,
    pub set: Vec<UpdateSetElement>,
    pub remove: Vec<String>,
    pub where_: Option<Where>,
    pub upsert: bool,
    pub ratelimit: Option<RateLimit>,
    pub return_value: ReturnValue,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DeleteCmd {
    pub table: String,
    pub index: Option<String>,
    pub where_: Option<Where>,
    pub ratelimit: Option<RateLimit>,
    pub return_value: ReturnValue,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AlterTableCmd {
    pub table: String,
    pub billing: Option<BillingModeAndThroughput>,
    pub warm: Option<WarmThroughput>,
    pub sse: Option<SseSpec>,
    pub stream: Option<StreamSpec>,
    pub table_class: Option<TableClass>,
    pub deletion_protection: Option<bool>,
    pub attribute_definitions: Vec<AttributeDef>,
    pub gsi_updates: Vec<GsiUpdate>,
    pub vector_index_updates: Vec<VectorIndexUpdateDef>,
    pub ignored_gsi_on_demand: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AlterTtlCmd {
    pub table: String,
    pub enabled: bool,
    pub attribute: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AlterPitrCmd {
    pub table: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AlterReplicaCmd {
    pub table: String,
    pub replica: ReplicaUpdate,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct CreateBackupCmd {
    pub table: String,
    pub backup_name: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DropBackupCmd {
    pub backup_arn: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DescribeBackupCmd {
    pub backup_arn: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RestoreKind {
    #[default]
    FromBackup,
    ToPitr,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RestoreCmd {
    pub kind: RestoreKind,
    pub target: String,
    pub source: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ShowBackupsCmd {
    pub backup_type: String,
    pub table: String,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ShowCreateTableCmd {
    pub if_not_exists: bool,
    pub nowait: bool,
    pub table: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TransactAccess {
    #[default]
    Read,
    Write,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TransactCmd {
    pub access: TransactAccess,
    /// ROLLBACK terminator (vs COMMIT)
    pub rollback: bool,
    pub items: Vec<CommandNode>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum CommandNode {
    #[default]
    Empty,
    Quit,
    ShowVersion,
    ShowConnection,
    Connect(ConnectCmd),
    ShowTables(ShowTablesCmd),
    Describe(DescribeCmd),
    DropTable(DropTableCmd),
    CreateTable(CreateTableCmd),
    AlterTable(AlterTableCmd),
    AlterTtl(AlterTtlCmd),
    AlterPitr(AlterPitrCmd),
    AlterReplica(AlterReplicaCmd),
    Insert(InsertCmd),
    Select(SelectCmd),
    Update(UpdateCmd),
    Delete(DeleteCmd),
    Help(HelpCmd),
    Transact(TransactCmd),
    CreateBackup(CreateBackupCmd),
    DropBackup(DropBackupCmd),
    DescribeBackup(DescribeBackupCmd),
    Restore(RestoreCmd),
    ShowBackups(ShowBackupsCmd),
    ShowLimits,
    ShowCreateTable(ShowCreateTableCmd),
}

impl CommandNode {
    pub fn kind(&self) -> StatementKind {
        use CommandNode as C;
        use StatementKind as K;
        match self {
            C::Empty => K::Empty,
            C::Quit => K::Quit,
            C::ShowVersion => K::ShowVersion,
            C::ShowConnection => K::ShowConnection,
            C::Connect(_) => K::Connect,
            C::ShowTables(_) => K::ShowTables,
            C::Describe(_) => K::Describe,
            C::DropTable(_) => K::DropTable,
            C::CreateTable(_) => K::CreateTable,
            C::AlterTable(_) | C::AlterTtl(_) | C::AlterPitr(_) | C::AlterReplica(_) => {
                K::AlterTable
            }
            C::Insert(_) => K::Insert,
            C::Select(_) => K::SelectStatement,
            C::Update(_) => K::UpdateStatement,
            C::Delete(_) => K::Del,
            C::Help(_) => K::Help,
            C::Transact(_) => K::Transaction,
            C::CreateBackup(_) => K::CreateBackup,
            C::DropBackup(_) => K::DropBackup,
            C::DescribeBackup(_) => K::DescribeBackup,
            C::Restore(_) => K::Restore,
            C::ShowBackups(_) => K::ShowBackups,
            C::ShowLimits => K::ShowLimits,
            C::ShowCreateTable(_) => K::ShowCreateTable,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ParsedStatement {
    pub command: CommandNode,
    /// the EXPLAIN prefix was present
    pub explain: bool,
}

impl ParsedStatement {
    pub fn kind(&self) -> StatementKind {
        self.command.kind()
    }
}
