// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! Unit tests for the DDL executors against an in-memory fake client.
use std::collections::HashMap;

use crate::client::*;
use crate::error::{Errc, Error, Result};
use crate::outcome::ExecOutcome;
use crate::parser::ast::*;
use crate::render::Renderer;

use super::alter_table_executor::*;
use super::backup_restore_executor::*;
use super::create_table_executor::*;
use super::describe_executor::*;
use super::drop_table_executor::*;
use super::show_commands_executor::*;
use super::show_tables_executor::*;

#[derive(Default)]
struct Fake {
    tables: HashMap<String, DescribeTableOut>,
    table_names: Vec<String>,
    ttl: HashMap<String, TimeToLiveOut>,
    pitr: Option<ContinuousBackupsOut>,
    replicas: Vec<ReplicaAutoScalingDescription>,
    limits: DescribeLimitsOut,
    backup_pages: Vec<ListBackupsOut>,
    list_backups_calls: Vec<ListBackupsIn>,
    calls: Vec<String>,
    created: Vec<CreateTableIn>,
    updated: Vec<UpdateTableIn>,
    ttl_updates: Vec<UpdateTimeToLiveIn>,
    create_error: Option<Error>,
}

fn not_found(name: &str) -> Error {
    Error::new(Errc::NoSuchTable, format!("Table: {} not found", name))
}

impl DdbClient for Fake {
    fn get_item(&mut self, _: &GetItemIn) -> Result<GetItemOut> {
        unimplemented!()
    }
    fn query(&mut self, _: &QueryIn) -> Result<QueryOut> {
        unimplemented!()
    }
    fn scan(&mut self, _: &ScanIn) -> Result<ScanOut> {
        unimplemented!()
    }
    fn put_item(&mut self, _: &PutItemIn) -> Result<PutItemOut> {
        unimplemented!()
    }
    fn create_table(&mut self, i: &CreateTableIn) -> Result<()> {
        self.calls.push("CreateTable".into());
        if let Some(e) = self.create_error.clone() {
            return Err(e);
        }
        self.created.push(i.clone());
        Ok(())
    }
    fn update_table(&mut self, i: &UpdateTableIn) -> Result<()> {
        self.calls.push("UpdateTable".into());
        self.updated.push(i.clone());
        Ok(())
    }
    fn update_time_to_live(&mut self, i: &UpdateTimeToLiveIn) -> Result<()> {
        self.calls.push("UpdateTimeToLive".into());
        self.ttl_updates.push(i.clone());
        Ok(())
    }
    fn update_continuous_backups(&mut self, _: &UpdateContinuousBackupsIn) -> Result<()> {
        self.calls.push("UpdateContinuousBackups".into());
        Ok(())
    }
    fn create_backup(&mut self, _: &CreateBackupIn) -> Result<()> {
        Ok(())
    }
    fn delete_backup(&mut self, _: &DeleteBackupIn) -> Result<()> {
        Ok(())
    }
    fn describe_backup(&mut self, i: &DescribeBackupIn) -> Result<DescribeBackupOut> {
        Ok(DescribeBackupOut {
            source_table_name: "t".into(),
            source_table_id: "tid".into(),
            source_table_arn: "tarn".into(),
            table_size_bytes: 10,
            item_count: 2,
            key_schema: vec![
                BackupKeySchemaElement {
                    key_type: "HASH".into(),
                    attribute_name: "a".into(),
                },
                BackupKeySchemaElement {
                    key_type: "RANGE".into(),
                    attribute_name: "b".into(),
                },
            ],
            table_creation_datetime: "T0".into(),
            billing_mode: Some("PROVISIONED".into()),
            provisioned: Some((5, 6)),
            backup_name: "bk".into(),
            backup_arn: i.backup_arn.clone(),
            backup_size_bytes: 11,
            backup_status: "AVAILABLE".into(),
            backup_type: "USER".into(),
            backup_creation_datetime: "T1".into(),
            backup_expiry_datetime: "T2".into(),
        })
    }
    fn restore_table_from_backup(&mut self, _: &RestoreFromBackupIn) -> Result<()> {
        Ok(())
    }
    fn restore_table_to_point_in_time(&mut self, _: &RestoreToPitrIn) -> Result<()> {
        Ok(())
    }
    fn describe_limits(&mut self, _: &DescribeLimitsIn) -> Result<DescribeLimitsOut> {
        Ok(self.limits.clone())
    }
    fn list_backups(&mut self, i: &ListBackupsIn) -> Result<ListBackupsOut> {
        self.list_backups_calls.push(i.clone());
        Ok(if self.backup_pages.is_empty() {
            ListBackupsOut::default()
        } else {
            self.backup_pages.remove(0)
        })
    }
    fn delete_table(&mut self, i: &DeleteTableIn) -> Result<()> {
        self.calls.push("DeleteTable".into());
        self.tables
            .remove(&i.table_name)
            .map(|_| ())
            .ok_or_else(|| not_found(&i.table_name))
    }
    fn list_tables(&mut self, _: &ListTablesIn) -> Result<ListTablesOut> {
        Ok(ListTablesOut {
            table_names: self.table_names.clone(),
        })
    }
    fn describe_table(&mut self, i: &DescribeTableIn) -> Result<DescribeTableOut> {
        self.tables
            .get(&i.table_name)
            .cloned()
            .ok_or_else(|| not_found(&i.table_name))
    }
    fn update_item(&mut self, _: &UpdateItemIn) -> Result<UpdateItemOut> {
        unimplemented!()
    }
    fn delete_item(&mut self, _: &DeleteItemIn) -> Result<DeleteItemOut> {
        unimplemented!()
    }
    fn transact_get_items(&mut self, _: &TransactGetItemsIn) -> Result<TransactGetItemsOut> {
        unimplemented!()
    }
    fn transact_write_items(&mut self, _: &TransactWriteItemsIn) -> Result<()> {
        unimplemented!()
    }
    fn describe_time_to_live(&mut self, i: &DescribeTimeToLiveIn) -> Result<TimeToLiveOut> {
        Ok(self
            .ttl
            .get(&i.table_name)
            .cloned()
            .unwrap_or(TimeToLiveOut {
                status: "DISABLED".into(),
                attribute_name: String::new(),
            }))
    }
    fn describe_continuous_backups(
        &mut self,
        _: &DescribeContinuousBackupsIn,
    ) -> Result<ContinuousBackupsOut> {
        Ok(self.pitr.clone().unwrap_or(ContinuousBackupsOut {
            pitr_status: "DISABLED".into(),
            ..Default::default()
        }))
    }
    fn describe_table_replica_auto_scaling(
        &mut self,
        _: &DescribeTableReplicaAutoScalingIn,
    ) -> Result<TableReplicaAutoScalingOut> {
        Ok(TableReplicaAutoScalingOut {
            replicas: self.replicas.clone(),
        })
    }
    fn wait_for_table_active(&mut self, _: &str) -> Result<()> {
        self.calls.push("WaitActive".into());
        Ok(())
    }
    fn wait_for_table_gone(&mut self, _: &str) -> Result<()> {
        self.calls.push("WaitGone".into());
        Ok(())
    }
    fn explain_request(&mut self, r: &ExplainRequest) -> Result<String> {
        Ok(match r {
            ExplainRequest::DescribeTable(i) => {
                format!("{{\n\t\"TableName\":\t\"{}\"\n}}", i.table_name)
            }
            other => format!(
                "{{\n\t\"Req\":\t\"{}\"\n}}",
                format!("{:?}", other).split('(').next().unwrap()
            ),
        })
    }
}

fn render(o: &ExecOutcome) -> String {
    Renderer.render(o)
}

fn base_provisioned(name: &str) -> DescribeTableOut {
    DescribeTableOut {
        table_name: name.into(),
        table_status: "ACTIVE".into(),
        key_schema: TableKeySchema {
            hash_keys: vec!["pk".into()],
            range_keys: vec!["rk".into()],
        },
        attribute_definitions: vec![
            AttributeDef {
                name: "pk".into(),
                ty: ScalarType::N,
            },
            AttributeDef {
                name: "rk".into(),
                ty: ScalarType::S,
            },
        ],
        creation_date_time: "2026-10-13T00:00:00Z".into(),
        table_arn: "arn:t".into(),
        table_id: "id-1".into(),
        table_size_bytes: 1024,
        item_count: 7,
        billing_mode_summary: Some("PROVISIONED".into()),
        provisioned_throughput_set: true,
        provisioned_read_capacity_units: 10,
        provisioned_write_capacity_units: 20,
        table_class_summary: Some("STANDARD".into()),
        ..Default::default()
    }
}

fn fake_with(t: DescribeTableOut) -> Fake {
    let mut f = Fake::default();
    f.table_names.push(t.table_name.clone());
    f.tables.insert(t.table_name.clone(), t);
    f
}

#[test]
fn describe_provisioned_basic_report() {
    let mut c = fake_with(base_provisioned("t1"));
    let out = execute_describe(&DescribeCmd { table: "t1".into() }, &mut c).unwrap();
    assert_eq!(
        render(&out),
        "Name: t1 (ACTIVE)\n\
         Key: HASH pk, RANGE rk\n\
         Attributes:  pk, N,  rk, S\n\
         Created at: 2026-10-13T00:00:00Z\n\
         Table ARN: arn:t\n\
         Table ID: id-1\n\
         Table size (bytes): 1024\n\
         Item Count: 7\n\
         Deletion Protection: Disabled\n\
         Billing Mode: Provisioned (10 RCU, 20 WCU)\n\
         PITR is Disabled.\n\
         GSI: None\n\
         LSI: None\n\
         VSI: None\n\
         Stream: Disabled\n\
         Table Class: STANDARD\n\
         SSE: Not set\n"
    );
}

#[test]
fn describe_on_demand_gsi_and_replicas() {
    let mut t = base_provisioned("od");
    t.billing_mode_summary = Some("PAY_PER_REQUEST".into());
    t.on_demand_throughput_set = true;
    t.on_demand_max_read_request_units = 100;
    t.on_demand_max_write_request_units = 50;
    t.global_secondary_indexes.push(SecondaryIndexSchema {
        index_name: "g".into(),
        key_schema: TableKeySchema {
            hash_keys: vec!["x".into()],
            range_keys: vec![],
        },
        projection_set: true,
        projection: Projection {
            ty: ProjectionType::Include,
            non_key_attributes: vec!["a".into(), "b".into()],
        },
        index_status: "ACTIVE".into(),
        ..Default::default()
    });
    let mut c = fake_with(t);
    c.pitr = Some(ContinuousBackupsOut {
        pitr_status: "ENABLED".into(),
        earliest_restorable_date_time: "E".into(),
        latest_restorable_date_time: "L".into(),
    });
    c.replicas.push(ReplicaAutoScalingDescription {
        region_name: "us-west-2".into(),
        replica_status: "ACTIVE".into(),
    });
    let text = render(&execute_describe(&DescribeCmd { table: "od".into() }, &mut c).unwrap());
    assert!(text.contains("Billing Mode: On Demand\nOn Demand Throughput: (100 RCU, 50 WCU)\n"));
    assert!(text.contains("PITR is Enabled: [E to L]\n"));
    assert!(text.contains(
        "GSI g: ( HASH x ), Billing Mode: On Demand (mirrors table), Projecting (INCLUDE) (a, b), Status: ACTIVE, Backfilling: NO\n"
    ));
    assert!(text.ends_with("Replica Region: us-west-2 (Status: ACTIVE)\n"));
}

#[test]
fn describe_missing_table_errors() {
    let mut c = Fake::default();
    let e = execute_describe(
        &DescribeCmd {
            table: "nope".into(),
        },
        &mut c,
    )
    .unwrap_err();
    assert_eq!(e.code, Errc::NoSuchTable);
}

#[test]
fn describe_explain_blocks() {
    let mut c = Fake::default();
    let out =
        render(&execute_describe_explain(&DescribeCmd { table: "t".into() }, &mut c).unwrap());
    assert!(
        out.starts_with("DescribeTable({ \"TableName\": \"t\" })\nDescribeTimeToLive({ \"Req\": ")
    );
    assert!(out.contains("DescribeContinuousBackups({\n   \"Req\":   "));
}

#[test]
fn show_tables_full_and_brief() {
    let mut c = fake_with(base_provisioned("alpha"));
    c.tables.insert("beta".into(), base_provisioned("beta"));
    c.table_names.push("beta".into());
    c.ttl.insert(
        "alpha".into(),
        TimeToLiveOut {
            status: "ENABLED".into(),
            attribute_name: "exp".into(),
        },
    );
    let out = render(
        &execute_show_tables(
            &ShowTablesCmd {
                brief: false,
                like: String::new(),
            },
            &mut c,
        )
        .unwrap(),
    );
    assert_eq!(
        out,
        "alpha | ACTIVE | PROVISIONED | STANDARD | id-1 | arn:t | TTL ENABLED (exp) | GSI: 0 | LSI : 0 | VSI : 0 |\n\
         beta | ACTIVE | PROVISIONED | STANDARD | id-1 | arn:t | TTL DISABLED | GSI: 0 | LSI : 0 | VSI : 0 |\n"
    );
    let out = render(
        &execute_show_tables(
            &ShowTablesCmd {
                brief: true,
                like: "b.*".into(),
            },
            &mut c,
        )
        .unwrap(),
    );
    assert_eq!(out, "beta\n");
    // LIKE is a full match.
    let out = render(
        &execute_show_tables(
            &ShowTablesCmd {
                brief: true,
                like: "lph".into(),
            },
            &mut c,
        )
        .unwrap(),
    );
    assert_eq!(out, "");
}

#[test]
fn drop_table_waits_and_if_exists() {
    let mut c = fake_with(base_provisioned("t"));
    let out = execute_drop_table(
        &DropTableCmd {
            table: "t".into(),
            ..Default::default()
        },
        &mut c,
    )
    .unwrap();
    assert_eq!(render(&out), "DROP\n");
    assert_eq!(c.calls, vec!["DeleteTable", "WaitGone"]);
    let out = execute_drop_table(
        &DropTableCmd {
            table: "t".into(),
            if_exists: true,
            nowait: false,
        },
        &mut c,
    )
    .unwrap();
    assert_eq!(out, ExecOutcome::Empty);
    let e = execute_drop_table(
        &DropTableCmd {
            table: "t".into(),
            ..Default::default()
        },
        &mut c,
    )
    .unwrap_err();
    assert_eq!(e.message, "Table: t not found");
}

fn simple_create(name: &str) -> CreateTableCmd {
    CreateTableCmd {
        table_name: name.into(),
        attribute_definitions: vec![AttributeDef {
            name: "a".into(),
            ty: ScalarType::N,
        }],
        key_schema: KeySchema {
            hash: vec!["a".into()],
            range: vec![],
        },
        ..Default::default()
    }
}

#[test]
fn create_table_waits_and_if_not_exists() {
    let mut c = Fake::default();
    let out = execute_create_table(&simple_create("t"), &mut c).unwrap();
    assert_eq!(render(&out), "CREATE\n");
    assert_eq!(c.calls, vec!["CreateTable", "WaitActive"]);
    assert_eq!(c.created[0].billing, None);

    let mut c = fake_with(base_provisioned("t"));
    let mut cmd = simple_create("t");
    cmd.if_not_exists = true;
    assert_eq!(
        render(&execute_create_table(&cmd, &mut c).unwrap()),
        "CREATE\n"
    );
    assert!(c.calls.is_empty());

    let mut c = Fake::default();
    let mut cmd = simple_create("t");
    cmd.nowait = true;
    execute_create_table(&cmd, &mut c).unwrap();
    assert_eq!(c.calls, vec!["CreateTable"]);

    let mut c = Fake {
        create_error: Some(Error::new(
            Errc::TableAlreadyExists,
            "Table already exists: t",
        )),
        ..Default::default()
    };
    assert_eq!(
        execute_create_table(&simple_create("t"), &mut c)
            .unwrap_err()
            .message,
        "Table already exists: t"
    );
}

#[test]
fn create_table_validation() {
    let mut c = Fake::default();
    let mut cmd = simple_create("t");
    cmd.key_schema.hash.push("b".into());
    assert_eq!(
        execute_create_table(&cmd, &mut c).unwrap_err().message,
        "Composite keys are supported only on GSIs. A table primary key must have one partition key and an optional sort key."
    );
    let mut cmd = simple_create("t");
    cmd.key_schema.hash.clear();
    assert_eq!(
        execute_create_table(&cmd, &mut c).unwrap_err().message,
        "A table primary key must specify a partition (HASH) key."
    );
    let mut cmd = simple_create("t");
    cmd.gsis.push(Gsi {
        name: "g".into(),
        key_schema: KeySchema {
            hash: vec!["1", "2", "3", "4", "5"]
                .into_iter()
                .map(String::from)
                .collect(),
            range: vec![],
        },
        ..Default::default()
    });
    assert_eq!(
        execute_create_table(&cmd, &mut c).unwrap_err().message,
        "GSI g key schema exceeds the 4-attribute limit per partition/sort key."
    );
    let mut cmd = simple_create("t");
    cmd.vector_indexes.push(VectorIndexDef {
        index_name: "v".into(),
        vector_attr: "a".into(),
        ..Default::default()
    });
    assert_eq!(
        execute_create_table(&cmd, &mut c).unwrap_err().message,
        "The vector attribute a of vector index v must not be declared in the attribute list (it is a vector, not a scalar key)."
    );
    assert!(c.calls.is_empty());
}

#[test]
fn create_table_explain_has_trailing_create() {
    let mut c = Fake::default();
    let out = render(&execute_create_table_explain(&simple_create("t"), &mut c).unwrap());
    assert!(out.starts_with("CreateTable({\n"));
    assert!(out.ends_with("})\nCREATE\n"));
}

#[test]
fn alter_table_on_demand_diagnostic_and_ttl() {
    let mut c = Fake::default();
    let cmd = AlterTableCmd {
        table: "t".into(),
        ignored_gsi_on_demand: true,
        ..Default::default()
    };
    let out = execute_alter_table(&cmd, &mut c).unwrap();
    assert_eq!(render(&out), "ALTER\n");
    assert_eq!(
        Renderer.render_stderr(&out),
        "Setting table to on-demand, ignoring specifications for indices.\n"
    );

    // Disabling an already-disabled TTL is a no-op.
    let out = execute_alter_ttl(
        &AlterTtlCmd {
            table: "t".into(),
            enabled: false,
            attribute: String::new(),
        },
        &mut c,
        false,
    )
    .unwrap();
    assert_eq!(render(&out), "ALTER\n");
    assert!(c.ttl_updates.is_empty());
    // Disabling an enabled TTL reuses the current attribute name.
    c.ttl.insert(
        "t".into(),
        TimeToLiveOut {
            status: "ENABLED".into(),
            attribute_name: "exp".into(),
        },
    );
    execute_alter_ttl(
        &AlterTtlCmd {
            table: "t".into(),
            enabled: false,
            attribute: String::new(),
        },
        &mut c,
        false,
    )
    .unwrap();
    assert_eq!(c.ttl_updates[0].attribute_name, "exp");

    let out = render(
        &execute_alter_pitr(
            &AlterPitrCmd {
                table: "t".into(),
                enabled: true,
            },
            &mut c,
            true,
        )
        .unwrap(),
    );
    assert!(out.starts_with("UpdateContinuousBackupsRequest({"));
}

#[test]
fn backups() {
    let mut c = Fake::default();
    c.backup_pages = vec![
        ListBackupsOut {
            summaries: vec![BackupSummaryRow {
                table_name: "t".into(),
                backup_name: "b1".into(),
                backup_status: "AVAILABLE".into(),
                backup_arn: "arn1".into(),
                creation_datetime: "C".into(),
                expiry_datetime: "X".into(),
            }],
            last_evaluated_backup_arn: Some("arn1".into()),
        },
        ListBackupsOut::default(),
    ];
    let out = render(
        &execute_show_backups(
            &ShowBackupsCmd {
                backup_type: "USER".into(),
                table: "t".into(),
            },
            &mut c,
        )
        .unwrap(),
    );
    assert_eq!(
        out,
        "Table: t, Backup: b1, Status: AVAILABLE, ARN: arn1, On: C, Expires: X\n"
    );
    assert_eq!(c.list_backups_calls.len(), 2);
    assert_eq!(c.list_backups_calls[0].backup_type.as_deref(), Some("USER"));
    assert_eq!(
        c.list_backups_calls[1]
            .exclusive_start_backup_arn
            .as_deref(),
        Some("arn1")
    );

    let out = render(
        &execute_describe_backup(
            &DescribeBackupCmd {
                backup_arn: "barn".into(),
            },
            &mut c,
            false,
        )
        .unwrap(),
    );
    assert_eq!(
        out,
        "Table Name: t\nTable Id: tid\nTable Arn: tarn\nTable Size (bytes): 10\nItem Count: 2\n\
         Table Key: ( HASH a, RANGE b )\nTable Creation Date/Time: T0\nBilling Mode: PROVISIONED\n\
         Provisioned Throughput: 5 RCU, 6 WCU\nBackup Name: bk\nBackup Arn: barn\nBackup Size (bytes): 11\n\
         Backup Status: AVAILABLE\nBackup Type: USER\nBackup Creation Date/Time: T1\nBackup Expiry Date/Time: T2\n"
    );
    assert_eq!(
        render(
            &execute_create_backup(
                &CreateBackupCmd {
                    table: "t".into(),
                    backup_name: "b".into()
                },
                &mut c,
                false
            )
            .unwrap()
        ),
        "BACKUP\n"
    );
    assert_eq!(
        render(
            &execute_drop_backup(
                &DropBackupCmd {
                    backup_arn: "a".into()
                },
                &mut c,
                false
            )
            .unwrap()
        ),
        "DROP BACKUP\n"
    );
    let r = RestoreCmd {
        kind: RestoreKind::ToPitr,
        target: "t2".into(),
        source: "t".into(),
        timestamp: "ts".into(),
    };
    assert!(render(&execute_restore(&r, &mut c, true).unwrap())
        .starts_with("RestoreTableToPointInTimeRequest({"));
    assert_eq!(
        render(&execute_restore(&r, &mut c, false).unwrap()),
        "RESTORE\n"
    );
}

#[test]
fn show_limits_totals() {
    let mut t1 = DescribeTableOut {
        table_name: "t1".into(),
        table_status: "ACTIVE".into(),
        billing_mode_summary: Some("PROVISIONED".into()),
        provisioned_read_capacity_units: 10,
        provisioned_write_capacity_units: 5,
        ..Default::default()
    };
    t1.global_secondary_indexes.push(SecondaryIndexSchema {
        index_name: "g".into(),
        provisioned_read_capacity_units: 4,
        provisioned_write_capacity_units: 2,
        ..Default::default()
    });
    let mut c = fake_with(t1);
    let t2 = DescribeTableOut {
        table_name: "t2".into(),
        table_status: "ACTIVE".into(),
        billing_mode_summary: Some("PAY_PER_REQUEST".into()),
        ..Default::default()
    };
    c.tables.insert("t2".into(), t2);
    c.table_names.push("t2".into());
    c.limits = DescribeLimitsOut {
        account_max_rcu: 40000,
        account_max_wcu: 40000,
        table_max_rcu: 40000,
        table_max_wcu: 40000,
    };
    assert_eq!(
        render(&execute_show_limits(&mut c, "us-east-1").unwrap()),
        "Region: us-east-1\nAccount Max (RCU, WCU): 40000, 40000\nTable Max (RCU, WCU):  40000, 40000\n\
         Total Tables (RCU, WCU): 10, 5\nTotal GSI (RCU, WCU): 4, 2\nTotal (RCU, WCU): 14, 7\n"
    );
}

#[test]
fn show_create_table_variants() {
    let td = DescribeTableOut {
        table_name: "t".into(),
        table_status: "ACTIVE".into(),
        attribute_definitions: vec![
            AttributeDef {
                name: "id".into(),
                ty: ScalarType::S,
            },
            AttributeDef {
                name: "n".into(),
                ty: ScalarType::N,
            },
        ],
        key_schema: TableKeySchema {
            hash_keys: vec!["id".into()],
            range_keys: vec![],
        },
        billing_mode_summary: Some("PROVISIONED".into()),
        provisioned_read_capacity_units: 5,
        provisioned_write_capacity_units: 3,
        ..Default::default()
    };
    let mut c = fake_with(td);
    assert_eq!(
        render(&execute_show_create_table(&ShowCreateTableCmd { table: "t".into(), ..Default::default() }, &mut c).unwrap()),
        "CREATE TABLE \"t\" (\"id\" string, \"n\" number) PRIMARY KEY (\"id\" HASH) \
         BILLING MODE PROVISIONED ( 5 RCU, 3 WCU ) TABLE CLASS STANDARD DELETION PROTECTION DISABLED\n"
    );

    let mut td = DescribeTableOut {
        table_name: "t".into(),
        table_status: "ACTIVE".into(),
        attribute_definitions: vec![
            AttributeDef {
                name: "pk".into(),
                ty: ScalarType::S,
            },
            AttributeDef {
                name: "sk".into(),
                ty: ScalarType::N,
            },
            AttributeDef {
                name: "gpk".into(),
                ty: ScalarType::S,
            },
        ],
        key_schema: TableKeySchema {
            hash_keys: vec!["pk".into()],
            range_keys: vec!["sk".into()],
        },
        billing_mode_summary: Some("PAY_PER_REQUEST".into()),
        ..Default::default()
    };
    td.global_secondary_indexes.push(SecondaryIndexSchema {
        index_name: "g1".into(),
        key_schema: TableKeySchema {
            hash_keys: vec!["gpk".into()],
            range_keys: vec![],
        },
        projection_set: true,
        projection: Projection {
            ty: ProjectionType::Include,
            non_key_attributes: vec!["a".into(), "b".into()],
        },
        ..Default::default()
    });
    let mut c = fake_with(td);
    let cmd = ShowCreateTableCmd {
        table: "t".into(),
        if_not_exists: true,
        nowait: true,
    };
    assert_eq!(
        render(&execute_show_create_table(&cmd, &mut c).unwrap()),
        "CREATE TABLE IF NOT EXISTS NOWAIT \"t\" (\"pk\" string, \"sk\" number, \"gpk\" string) \
         PRIMARY KEY (\"pk\" HASH, \"sk\" RANGE) BILLING MODE ON DEMAND \
         GSI (\"g1\" ON ( \"gpk\" HASH ) PROJECTING INCLUDE (a, b) BILLING MODE ON DEMAND ) \
         TABLE CLASS STANDARD DELETION PROTECTION DISABLED\n"
    );

    let mut c = Fake::default();
    let e = execute_show_create_table(
        &ShowCreateTableCmd {
            table: "x".into(),
            ..Default::default()
        },
        &mut c,
    )
    .unwrap_err();
    assert_eq!(e.message, "The table \"x\" does not exist.");
    let mut t = base_provisioned("c");
    t.table_status = "CREATING".into();
    let mut c = fake_with(t);
    let e = execute_show_create_table(
        &ShowCreateTableCmd {
            table: "c".into(),
            ..Default::default()
        },
        &mut c,
    )
    .unwrap_err();
    assert_eq!(e.message, "The table \"c\" is not ACTIVE.");
}

#[test]
fn composite_gsi_key_description() {
    let mut td = base_provisioned("t");
    td.billing_mode_summary = Some("PAY_PER_REQUEST".into());
    td.global_secondary_indexes.push(SecondaryIndexSchema {
        index_name: "g".into(),
        key_schema: TableKeySchema {
            hash_keys: vec!["a".into(), "b".into()],
            range_keys: vec!["c".into(), "d".into()],
        },
        projection_set: true,
        ..Default::default()
    });
    let mut c = fake_with(td);
    let ddl = render(
        &execute_show_create_table(
            &ShowCreateTableCmd {
                table: "t".into(),
                ..Default::default()
            },
            &mut c,
        )
        .unwrap(),
    );
    assert!(
        ddl.contains("GSI (\"g\" ON ( (\"a\", \"b\") HASH, (\"c\", \"d\") RANGE ) PROJECTING ALL"),
        "{}",
        ddl
    );
}

// ---- CLI driver exit-code semantics (port of cli_exit_code_test.cpp) ------

fn run_cli(source: &str, abort_on_error: bool) -> (i32, String, String) {
    use crate::cli::{run_source_with, StreamLineSource, Terminator};
    use crate::outcome::BufferingSink;
    use crate::session::{Session, SessionConfig};
    let mut session = Session::new(
        SessionConfig {
            region: "us-east-1".into(),
            endpoint: String::new(),
        },
        Box::new(Fake::default()),
    );
    let mut src = StreamLineSource::new(source.as_bytes());
    let mut term = Terminator::default();
    let mut sink = BufferingSink::default();
    let mut errors = String::new();
    let rc = run_source_with(
        &mut session,
        &Renderer,
        &mut src,
        &mut term,
        abort_on_error,
        &mut sink,
        &mut |e: &str| errors.push_str(e),
    );
    let out: String = sink.parts.iter().map(|p| Renderer.render(p)).collect();
    (rc, out, errors)
}

const FAIL: &str = "describe nope;";
const OK: &str = "drop table if exists gone;";

#[test]
fn cli_exit_codes_last_executed_statement() {
    assert_eq!(run_cli(&format!("{}\n{}\n", FAIL, OK), false).0, 0);
    assert_eq!(run_cli(&format!("{}\n{}\n", OK, FAIL), false).0, 1);
    assert_eq!(run_cli(&format!("{}\n", FAIL), false).0, 1);
    assert_eq!(run_cli(&format!("{}\n", OK), false).0, 0);
}

#[test]
fn cli_exit_codes_abort_on_error() {
    assert_eq!(run_cli(&format!("{}\n{}\n", FAIL, OK), true).0, 1);
    assert_eq!(run_cli(&format!("{}\n{}\n", OK, FAIL), true).0, 1);
    assert_eq!(run_cli(&format!("{}\n{}\n", OK, OK), true).0, 0);
}

#[test]
fn cli_parse_error_does_not_set_exit_code() {
    let (rc, _, errors) = run_cli("this is not valid sql;\n", false);
    assert_eq!(rc, 0);
    assert!(!errors.is_empty());
}

#[test]
fn cli_go_terminator_and_quit() {
    let (rc, out, _) = run_cli("show version\ngo\nquit\ngo\nshow version\n", false);
    assert_eq!(rc, 0);
    assert_eq!(out, format!("ddbsh - version {}\nQUIT\n", crate::version()));
    let (_, _, errors) = run_cli("describe nope;\n", false);
    assert_eq!(errors, "Table: nope not found\n");
}
