// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! SHOW TABLES (port of src/execute/show_tables_executor.cpp).
use crate::client::{
    DdbClient, DescribeTableIn, DescribeTableOut, DescribeTimeToLiveIn, ListTablesPageIn,
    TimeToLiveOut,
};
use crate::error::{fail, Errc, Result};
use crate::outcome::{ExecOutcome, OutcomeSink, ReportOutcome, ReportTable};
use crate::parser::ast::ShowTablesCmd;
use crate::value::{Value, ValueMap};

fn billing_mode_column(td: &DescribeTableOut) -> String {
    if let Some(b) = &td.billing_mode_summary {
        return b.clone();
    }
    if td.provisioned_throughput_set {
        return "PROVISIONED".into();
    }
    "PAY_PER_REQUEST*".into()
}

fn table_class_column(td: &DescribeTableOut) -> String {
    td.table_class_summary
        .clone()
        .unwrap_or_else(|| "STANDARD*".into())
}

fn ttl_column(ttl: &TimeToLiveOut) -> String {
    let mut out = ttl.status.clone();
    if ttl.status == "ENABLED" || ttl.status == "ENABLING" {
        out += &format!(" ({})", ttl.attribute_name);
    }
    out
}

const FULL_COLUMNS: [&str; 10] = [
    "TableName",
    "TableStatus",
    "BillingMode",
    "TableClass",
    "TableId",
    "TableArn",
    "TimeToLive",
    "GSICount",
    "LSICount",
    "VSICount",
];

/// One SHOW TABLES row: the text line, and the same values by column (for the
/// output formats). A row whose describe failed carries only TableName.
struct TableRow {
    line: String,
    values: ValueMap,
}

fn name_only(name: &str) -> TableRow {
    TableRow {
        line: name.to_string(),
        values: vec![("TableName".into(), Value::S(name.to_string()))],
    }
}

fn status_row(name: &str, td: &DescribeTableOut, ttl: &TimeToLiveOut) -> TableRow {
    let line = format!(
        "{} | {} | {} | {} | {} | {} | TTL {} | GSI: {} | LSI : {} | VSI : {} |",
        name,
        td.table_status,
        billing_mode_column(td),
        table_class_column(td),
        td.table_id,
        td.table_arn,
        ttl_column(ttl),
        td.global_secondary_indexes.len(),
        td.local_secondary_indexes.len(),
        td.vector_indexes.len()
    );
    let s = |v: String| Value::S(v);
    let n = |v: usize| Value::N(v.to_string());
    let values = vec![
        ("TableName".into(), s(name.to_string())),
        ("TableStatus".into(), s(td.table_status.clone())),
        ("BillingMode".into(), s(billing_mode_column(td))),
        ("TableClass".into(), s(table_class_column(td))),
        ("TableId".into(), s(td.table_id.clone())),
        ("TableArn".into(), s(td.table_arn.clone())),
        ("TimeToLive".into(), s(ttl_column(ttl))),
        ("GSICount".into(), n(td.global_secondary_indexes.len())),
        ("LSICount".into(), n(td.local_secondary_indexes.len())),
        ("VSICount".into(), n(td.vector_indexes.len())),
    ];
    TableRow { line, values }
}

/// Just the name for BRIEF, or when DescribeTable or DescribeTimeToLive fails;
/// else the full status row.
fn table_row(cmd: &ShowTablesCmd, client: &mut dyn DdbClient, name: &str) -> TableRow {
    if cmd.brief {
        return name_only(name);
    }
    let Ok(described) = client.describe_table(&DescribeTableIn {
        table_name: name.to_string(),
    }) else {
        return name_only(name);
    };
    let Ok(ttl) = client.describe_time_to_live(&DescribeTimeToLiveIn {
        table_name: name.to_string(),
    }) else {
        return name_only(name);
    };
    status_row(name, &described, &ttl)
}

/// One SHOW TABLES fragment: the row's line, and its values as a one-row
/// table. With no row, an empty page end (renders nothing by default).
fn fragment(cmd: &ShowTablesCmd, row: Option<TableRow>, group_end: bool) -> ExecOutcome {
    let columns: Vec<String> = if cmd.brief {
        vec!["TableName".into()]
    } else {
        FULL_COLUMNS.iter().map(|c| c.to_string()).collect()
    };
    let mut report = ReportOutcome::default();
    let mut table = ReportTable {
        columns,
        rows: vec![],
        group_end,
    };
    if let Some(row) = row {
        report.lines.push(row.line);
        table.rows.push(row.values);
    }
    report.table = Some(table);
    ExecOutcome::Report(report)
}

/// Reads ListTables one page at a time and emits each row as soon as its
/// describe calls return. A later failure leaves earlier rows emitted.
pub fn execute_show_tables(
    cmd: &ShowTablesCmd,
    client: &mut dyn DdbClient,
    sink: &mut dyn OutcomeSink,
) -> Result<()> {
    let matcher = if cmd.like.is_empty() {
        None
    } else {
        match regex::Regex::new(&format!("^(?:{})$", cmd.like)) {
            Ok(r) => Some(r),
            Err(e) => return fail(Errc::ParseError, format!("Invalid LIKE pattern: {}", e)),
        }
    };
    let mut page_in = ListTablesPageIn::default();
    loop {
        let page = client.list_tables_page(&page_in)?;
        let names = &page.table_names;
        let mut page_end_emitted = false;
        for (i, name) in names.iter().enumerate() {
            if let Some(m) = &matcher {
                if !m.is_match(name) {
                    continue;
                }
            }
            let last = i + 1 == names.len();
            let row = table_row(cmd, client, name);
            sink.emit(fragment(cmd, Some(row), last));
            page_end_emitted = last;
        }
        if !page_end_emitted {
            sink.emit(fragment(cmd, None, true));
        }
        page_in.exclusive_start_table_name = page.last_evaluated_table_name;
        if page_in.exclusive_start_table_name.is_none() {
            return Ok(());
        }
    }
}
