// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! SHOW BACKUPS / SHOW LIMITS / SHOW CREATE TABLE
//! (port of src/execute/show_commands_executor.cpp).
use super::describe_executor::vector_distance_name;
use crate::client::{
    DdbClient, DescribeLimitsIn, DescribeTableIn, DescribeTableOut, DescribeTimeToLiveIn,
    ListBackupsIn, ListTablesIn, SecondaryIndexSchema, TableKeySchema, VectorIndexSchema,
};
use crate::error::{fail, Errc, Result};
use crate::outcome::{ExecOutcome, OutcomeSink, ReportOutcome, ReportTable};
use crate::parser::ast::{
    Projection, ProjectionType, ScalarType, SearchSchemaKind, ShowBackupsCmd, ShowCreateTableCmd,
};
use crate::value::{Value, ValueMap};

fn key_description(ks: &TableKeySchema) -> String {
    let mut groups: Vec<(&Vec<String>, &str)> = Vec::new();
    if !ks.hash_keys.is_empty() {
        groups.push((&ks.hash_keys, "HASH"));
    }
    if !ks.range_keys.is_empty() {
        groups.push((&ks.range_keys, "RANGE"));
    }
    let mut out = String::new();
    let n = groups.len();
    for (ix, (attrs, kt)) in groups.into_iter().enumerate() {
        if ix != 0 {
            out.push(' ');
        }
        if attrs.len() == 1 {
            out += &format!("\"{}\" {}", attrs[0], kt);
        } else {
            let quoted: Vec<String> = attrs.iter().map(|a| format!("\"{}\"", a)).collect();
            out += &format!("({}) {}", quoted.join(", "), kt);
        }
        if ix != n - 1 {
            out.push(',');
        }
    }
    out
}

fn billing_mode_provisioned(rcu: i64, wcu: i64) -> String {
    format!("BILLING MODE PROVISIONED ( {} RCU, {} WCU ) ", rcu, wcu)
}

fn billing_mode_on_demand() -> String {
    "BILLING MODE ON DEMAND ".into()
}

fn projection_ddl(p: &Projection) -> String {
    match p.ty {
        ProjectionType::All => "PROJECTING ALL".into(),
        ProjectionType::KeysOnly => "PROJECTING KEYS ONLY".into(),
        ProjectionType::Include => {
            format!("PROJECTING INCLUDE ({})", p.non_key_attributes.join(", "))
        }
    }
}

fn show_gsis(gsis: &[SecondaryIndexSchema], table_on_demand: bool) -> String {
    if gsis.is_empty() {
        return String::new();
    }
    let mut out = String::from("GSI (");
    for (ix, g) in gsis.iter().enumerate() {
        if ix != 0 {
            out += ", ";
        }
        out += &format!(
            "\"{}\" ON ( {} ) ",
            g.index_name,
            key_description(&g.key_schema)
        );
        out += &projection_ddl(&g.projection);
        out.push(' ');
        if !table_on_demand {
            out += &billing_mode_provisioned(
                g.provisioned_read_capacity_units,
                g.provisioned_write_capacity_units,
            );
        } else {
            out += &billing_mode_on_demand();
        }
    }
    out += ") ";
    out
}

fn show_lsis(lsis: &[SecondaryIndexSchema]) -> String {
    if lsis.is_empty() {
        return String::new();
    }
    let mut out = String::from("LSI (");
    for (ix, l) in lsis.iter().enumerate() {
        if ix != 0 {
            out += ", ";
        }
        out += &format!(
            "\"{}\" ON ( {} ) ",
            l.index_name,
            key_description(&l.key_schema)
        );
        out += &projection_ddl(&l.projection);
        out.push(' ');
    }
    out += ") ";
    out
}

fn show_vsis(vsis: &[VectorIndexSchema]) -> String {
    if vsis.is_empty() {
        return String::new();
    }
    let mut out = String::from("VSI (");
    for (ix, vi) in vsis.iter().enumerate() {
        if ix != 0 {
            out += ", ";
        }
        out += &format!(
            "\"{}\" ON \"{}\" DIMENSIONS {} DISTANCE {}",
            vi.index_name,
            vi.vector_attr,
            vi.dimensions,
            vector_distance_name(vi.distance)
        );
        if !vi.search_schema.is_empty() {
            let parts: Vec<String> = vi
                .search_schema
                .iter()
                .map(|e| {
                    format!(
                        "\"{}\" {}",
                        e.attr,
                        if e.kind == SearchSchemaKind::Hash {
                            "HASH"
                        } else {
                            "FILTER"
                        }
                    )
                })
                .collect();
            out += &format!(" SEARCH KEY ( {} )", parts.join(", "));
        }
        out.push(' ');
        out += &if vi.projection_set {
            projection_ddl(&vi.projection)
        } else {
            "PROJECTING ALL".into()
        };
    }
    out += ") ";
    out
}

fn stream_view_words(mapped: &str) -> String {
    match mapped {
        "NEW_IMAGE" => "NEW IMAGE".into(),
        "OLD_IMAGE" => "OLD IMAGE".into(),
        "KEYS_ONLY" => "KEYS ONLY".into(),
        "NEW_AND_OLD_IMAGES" => "BOTH IMAGES".into(),
        other => other.into(),
    }
}

fn reconstruct_ddl(cmd: &ShowCreateTableCmd, td: &DescribeTableOut) -> String {
    let mut ctc = String::from("CREATE TABLE ");
    if cmd.if_not_exists {
        ctc += "IF NOT EXISTS ";
    }
    if cmd.nowait {
        ctc += "NOWAIT ";
    }
    ctc += &format!("\"{}\" (", td.table_name);
    let n = td.attribute_definitions.len();
    for (ix, a) in td.attribute_definitions.iter().enumerate() {
        if ix != 0 {
            ctc.push(' ');
        }
        ctc += &format!("\"{}\"", a.name);
        ctc += match a.ty {
            ScalarType::S => " string",
            ScalarType::N => " number",
            ScalarType::B => " binary",
        };
        if ix != n - 1 {
            ctc.push(',');
        }
    }
    ctc += ") PRIMARY KEY (";
    ctc += &key_description(&td.key_schema);
    ctc += ") ";
    let mut table_on_demand = false;
    if let Some(mode) = &td.billing_mode_summary {
        if mode == "PROVISIONED" {
            ctc += &billing_mode_provisioned(
                td.provisioned_read_capacity_units,
                td.provisioned_write_capacity_units,
            );
        } else if mode == "PAY_PER_REQUEST" {
            ctc += &billing_mode_on_demand();
            table_on_demand = true;
        }
    } else if td.provisioned_throughput_set {
        ctc += &billing_mode_provisioned(
            td.provisioned_read_capacity_units,
            td.provisioned_write_capacity_units,
        );
    }
    ctc += &show_gsis(&td.global_secondary_indexes, table_on_demand);
    ctc += &show_lsis(&td.local_secondary_indexes);
    ctc += &show_vsis(&td.vector_indexes);
    if td.stream_specification_set {
        match &td.stream_view_type {
            Some(v) => ctc += &format!("STREAM ( {} ) ", stream_view_words(v)),
            None => ctc += "STREAM DISABLED ",
        }
    }
    let table_class = td
        .table_class_summary
        .clone()
        .unwrap_or_else(|| "STANDARD".into());
    if table_class == "STANDARD_INFREQUENT_ACCESS" {
        ctc += "TABLE CLASS STANDARD INFREQUENT ACCESS ";
    } else {
        ctc += "TABLE CLASS STANDARD ";
    }
    ctc += if td.deletion_protection_enabled {
        "DELETION PROTECTION ENABLED"
    } else {
        "DELETION PROTECTION DISABLED"
    };
    ctc
}

pub fn execute_show_backups(
    cmd: &ShowBackupsCmd,
    client: &mut dyn DdbClient,
    sink: &mut dyn OutcomeSink,
) -> Result<()> {
    let mut request = ListBackupsIn {
        table_name: if cmd.table.is_empty() {
            None
        } else {
            Some(cmd.table.clone())
        },
        backup_type: if cmd.backup_type.is_empty() {
            None
        } else {
            Some(cmd.backup_type.clone())
        },
        limit: 100,
        exclusive_start_backup_arn: None,
    };
    // One fragment per non-empty ListBackups page, emitted before the next
    // page is requested. Each carries its rows by column for the output
    // formats; when no page has a backup, one empty fragment still goes out.
    let columns: Vec<String> = [
        "TableName",
        "BackupName",
        "BackupStatus",
        "BackupArn",
        "CreationDateTime",
        "ExpiryDateTime",
    ]
    .iter()
    .map(|c| c.to_string())
    .collect();
    let mut emitted = false;
    loop {
        let page = client.list_backups(&request)?;
        let mut report = ReportOutcome::default();
        let mut table = ReportTable {
            columns: columns.clone(),
            rows: vec![],
            group_end: true,
        };
        for s in &page.summaries {
            report.lines.push(format!(
                "Table: {}, Backup: {}, Status: {}, ARN: {}, On: {}, Expires: {}",
                s.table_name,
                s.backup_name,
                s.backup_status,
                s.backup_arn,
                s.creation_datetime,
                s.expiry_datetime
            ));
            table.rows.push(vec![
                ("TableName".into(), Value::S(s.table_name.clone())),
                ("BackupName".into(), Value::S(s.backup_name.clone())),
                ("BackupStatus".into(), Value::S(s.backup_status.clone())),
                ("BackupArn".into(), Value::S(s.backup_arn.clone())),
                (
                    "CreationDateTime".into(),
                    Value::S(s.creation_datetime.clone()),
                ),
                ("ExpiryDateTime".into(), Value::S(s.expiry_datetime.clone())),
            ]);
        }
        if !report.lines.is_empty() {
            report.table = Some(table);
            sink.emit(ExecOutcome::Report(report));
            emitted = true;
        }
        match page.last_evaluated_backup_arn {
            Some(arn) => request.exclusive_start_backup_arn = Some(arn),
            None => break,
        }
    }
    if !emitted {
        sink.emit(ExecOutcome::Report(ReportOutcome {
            lines: vec![],
            table: Some(ReportTable {
                columns,
                rows: vec![],
                group_end: true,
            }),
        }));
    }
    Ok(())
}

fn limit_row(region: &str, limit: &str, rcu: i64, wcu: i64) -> ValueMap {
    vec![
        ("Region".into(), Value::S(region.to_string())),
        ("Limit".into(), Value::S(limit.to_string())),
        ("RCU".into(), Value::N(rcu.to_string())),
        ("WCU".into(), Value::N(wcu.to_string())),
    ]
}

pub fn execute_show_limits(
    client: &mut dyn DdbClient,
    region: &str,
    sink: &mut dyn OutcomeSink,
) -> Result<()> {
    let limits = client.describe_limits(&DescribeLimitsIn {})?;
    let columns: Vec<String> = ["Region", "Limit", "RCU", "WCU"]
        .iter()
        .map(|c| c.to_string())
        .collect();
    // The header lines go out as soon as DescribeLimits returns, before the
    // ListTables/DescribeTable calls the totals need. Its rows do not end a
    // group, so the text table shows SHOW LIMITS as one box.
    sink.emit(ExecOutcome::Report(ReportOutcome {
        lines: vec![
            format!("Region: {}", region),
            format!(
                "Account Max (RCU, WCU): {}, {}",
                limits.account_max_rcu, limits.account_max_wcu
            ),
            format!(
                "Table Max (RCU, WCU):  {}, {}",
                limits.table_max_rcu, limits.table_max_wcu
            ),
        ],
        table: Some(ReportTable {
            columns: columns.clone(),
            rows: vec![
                limit_row(
                    region,
                    "Account Max",
                    limits.account_max_rcu,
                    limits.account_max_wcu,
                ),
                limit_row(
                    region,
                    "Table Max",
                    limits.table_max_rcu,
                    limits.table_max_wcu,
                ),
            ],
            group_end: false,
        }),
    }));
    let listed = client.list_tables(&ListTablesIn {})?;
    let (mut rcus, mut wcus, mut gsircus, mut gsiwcus) = (0i64, 0i64, 0i64, 0i64);
    for name in &listed.table_names {
        let td = match client.describe_table(&DescribeTableIn {
            table_name: name.clone(),
        }) {
            Ok(td) => td,
            Err(_) => continue,
        };
        let provisioned = td.billing_mode_summary.as_deref() == Some("PROVISIONED")
            || td.provisioned_throughput_set;
        if !provisioned {
            continue;
        }
        rcus += td.provisioned_read_capacity_units;
        wcus += td.provisioned_write_capacity_units;
        for gsi in &td.global_secondary_indexes {
            gsircus += gsi.provisioned_read_capacity_units;
            gsiwcus += gsi.provisioned_write_capacity_units;
        }
    }
    sink.emit(ExecOutcome::Report(ReportOutcome {
        lines: vec![
            format!("Total Tables (RCU, WCU): {}, {}", rcus, wcus),
            format!("Total GSI (RCU, WCU): {}, {}", gsircus, gsiwcus),
            format!("Total (RCU, WCU): {}, {}", rcus + gsircus, wcus + gsiwcus),
        ],
        table: Some(ReportTable {
            columns,
            rows: vec![
                limit_row(region, "Total Tables", rcus, wcus),
                limit_row(region, "Total GSI", gsircus, gsiwcus),
                limit_row(region, "Total", rcus + gsircus, wcus + gsiwcus),
            ],
            group_end: true,
        }),
    }));
    Ok(())
}

pub fn execute_show_create_table(
    cmd: &ShowCreateTableCmd,
    client: &mut dyn DdbClient,
) -> Result<ExecOutcome> {
    let td = match client.describe_table(&DescribeTableIn {
        table_name: cmd.table.clone(),
    }) {
        Ok(td) => td,
        Err(_) => {
            return fail(
                Errc::NoSuchTable,
                format!("The table \"{}\" does not exist.", cmd.table),
            )
        }
    };
    if td.table_status != "ACTIVE" {
        return fail(
            Errc::InvalidArgument,
            format!("The table \"{}\" is not ACTIVE.", cmd.table),
        );
    }
    let _ = client.describe_time_to_live(&DescribeTimeToLiveIn {
        table_name: cmd.table.clone(),
    });
    Ok(ExecOutcome::Report(ReportOutcome {
        lines: vec![reconstruct_ddl(cmd, &td)],
        table: None,
    }))
}
