// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! BACKUP / DROP BACKUP / DESCRIBE BACKUP / RESTORE
//! (port of src/execute/backup_restore_executor.cpp).
use crate::client::{
    CreateBackupIn, DdbClient, DeleteBackupIn, DescribeBackupIn, ExplainRequest,
    RestoreFromBackupIn, RestoreToPitrIn,
};
use crate::error::Result;
use crate::outcome::{ExecOutcome, ExplainBlock, ExplainFormat, ExplainOutcome, ReportOutcome};
use crate::parser::ast::{
    CreateBackupCmd, DescribeBackupCmd, DropBackupCmd, RestoreCmd, RestoreKind,
};

fn explain_one(client: &mut dyn DdbClient, api: &str, req: ExplainRequest) -> Result<ExecOutcome> {
    let body = client.explain_request(&req)?;
    Ok(ExecOutcome::Explain(ExplainOutcome {
        blocks: vec![ExplainBlock {
            api_name: api.into(),
            body,
            format: ExplainFormat::Pretty,
        }],
        trailing_status: None,
    }))
}

pub fn execute_create_backup(
    cmd: &CreateBackupCmd,
    client: &mut dyn DdbClient,
    explain: bool,
) -> Result<ExecOutcome> {
    let request = CreateBackupIn {
        table_name: cmd.table.clone(),
        backup_name: cmd.backup_name.clone(),
    };
    if explain {
        return explain_one(
            client,
            "CreateBackup",
            ExplainRequest::CreateBackup(request),
        );
    }
    client.create_backup(&request)?;
    Ok(ExecOutcome::status("BACKUP"))
}

pub fn execute_drop_backup(
    cmd: &DropBackupCmd,
    client: &mut dyn DdbClient,
    explain: bool,
) -> Result<ExecOutcome> {
    let request = DeleteBackupIn {
        backup_arn: cmd.backup_arn.clone(),
    };
    if explain {
        return explain_one(
            client,
            "DeleteBackup",
            ExplainRequest::DeleteBackup(request),
        );
    }
    client.delete_backup(&request)?;
    Ok(ExecOutcome::status("DROP BACKUP"))
}

pub fn execute_describe_backup(
    cmd: &DescribeBackupCmd,
    client: &mut dyn DdbClient,
    explain: bool,
) -> Result<ExecOutcome> {
    let request = DescribeBackupIn {
        backup_arn: cmd.backup_arn.clone(),
    };
    if explain {
        return explain_one(
            client,
            "DescribeBackup",
            ExplainRequest::DescribeBackup(request),
        );
    }
    let b = client.describe_backup(&request)?;
    let mut lines = vec![
        format!("Table Name: {}", b.source_table_name),
        format!("Table Id: {}", b.source_table_id),
        format!("Table Arn: {}", b.source_table_arn),
        format!("Table Size (bytes): {}", b.table_size_bytes),
        format!("Item Count: {}", b.item_count),
    ];
    let key: Vec<String> = b
        .key_schema
        .iter()
        .map(|k| format!("{} {}", k.key_type, k.attribute_name))
        .collect();
    lines.push(format!("Table Key: ( {} )", key.join(", ")));
    lines.push(format!(
        "Table Creation Date/Time: {}",
        b.table_creation_datetime
    ));
    if let Some(m) = &b.billing_mode {
        lines.push(format!("Billing Mode: {}", m));
    }
    if let Some((r, w)) = b.provisioned {
        lines.push(format!("Provisioned Throughput: {} RCU, {} WCU", r, w));
    }
    lines.push(format!("Backup Name: {}", b.backup_name));
    lines.push(format!("Backup Arn: {}", b.backup_arn));
    lines.push(format!("Backup Size (bytes): {}", b.backup_size_bytes));
    lines.push(format!("Backup Status: {}", b.backup_status));
    lines.push(format!("Backup Type: {}", b.backup_type));
    lines.push(format!(
        "Backup Creation Date/Time: {}",
        b.backup_creation_datetime
    ));
    lines.push(format!(
        "Backup Expiry Date/Time: {}",
        b.backup_expiry_datetime
    ));
    Ok(ExecOutcome::Report(ReportOutcome { lines, table: None }))
}

pub fn execute_restore(
    cmd: &RestoreCmd,
    client: &mut dyn DdbClient,
    explain: bool,
) -> Result<ExecOutcome> {
    if cmd.kind == RestoreKind::FromBackup {
        let request = RestoreFromBackupIn {
            target_table_name: cmd.target.clone(),
            backup_arn: cmd.source.clone(),
        };
        if explain {
            return explain_one(
                client,
                "RestoreTableFromBackup",
                ExplainRequest::RestoreFromBackup(request),
            );
        }
        client.restore_table_from_backup(&request)?;
        return Ok(ExecOutcome::status("RESTORE"));
    }
    let request = RestoreToPitrIn {
        source_table_name: cmd.source.clone(),
        target_table_name: cmd.target.clone(),
        restore_datetime: cmd.timestamp.clone(),
    };
    if explain {
        return explain_one(
            client,
            "RestoreTableToPointInTimeRequest",
            ExplainRequest::RestoreToPitr(request),
        );
    }
    client.restore_table_to_point_in_time(&request)?;
    Ok(ExecOutcome::status("RESTORE"))
}
