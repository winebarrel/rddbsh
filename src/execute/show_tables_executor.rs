//! SHOW TABLES (port of src/execute/show_tables_executor.cpp).
use crate::client::{
    DdbClient, DescribeTableIn, DescribeTableOut, DescribeTimeToLiveIn, ListTablesIn, TimeToLiveOut,
};
use crate::error::{fail, Errc, Result};
use crate::outcome::{ExecOutcome, ReportOutcome};
use crate::parser::ast::ShowTablesCmd;

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

fn status_row(name: &str, td: &DescribeTableOut, ttl: &TimeToLiveOut) -> String {
    format!(
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
    )
}

pub fn execute_show_tables(cmd: &ShowTablesCmd, client: &mut dyn DdbClient) -> Result<ExecOutcome> {
    let matcher = if cmd.like.is_empty() {
        None
    } else {
        match regex::Regex::new(&format!("^(?:{})$", cmd.like)) {
            Ok(r) => Some(r),
            Err(e) => return fail(Errc::ParseError, format!("Invalid LIKE pattern: {}", e)),
        }
    };
    let listed = client.list_tables(&ListTablesIn {})?;
    let mut report = ReportOutcome::default();
    for name in &listed.table_names {
        if let Some(m) = &matcher {
            if !m.is_match(name) {
                continue;
            }
        }
        if cmd.brief {
            report.lines.push(name.clone());
            continue;
        }
        let described = match client.describe_table(&DescribeTableIn {
            table_name: name.clone(),
        }) {
            Ok(d) => d,
            Err(_) => {
                report.lines.push(name.clone());
                continue;
            }
        };
        let ttl = match client.describe_time_to_live(&DescribeTimeToLiveIn {
            table_name: name.clone(),
        }) {
            Ok(t) => t,
            Err(_) => {
                report.lines.push(name.clone());
                continue;
            }
        };
        report.lines.push(status_row(name, &described, &ttl));
    }
    Ok(ExecOutcome::Report(report))
}
