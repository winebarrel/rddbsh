// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! ALTER TABLE / SET TTL / SET PITR / REPLICA (port of src/execute/alter_table_executor.cpp).
use crate::client::{
    DdbClient, DescribeTimeToLiveIn, ExplainRequest, UpdateContinuousBackupsIn, UpdateTableIn,
    UpdateTimeToLiveIn,
};
use crate::error::{fail, Errc, Result};
use crate::outcome::{ExecOutcome, ExplainBlock, ExplainFormat, ExplainOutcome, StatusOutcome};
use crate::parser::ast::{AlterPitrCmd, AlterReplicaCmd, AlterTableCmd, AlterTtlCmd, GsiAction};

const ON_DEMAND_GSI_IGNORED: &str =
    "Setting table to on-demand, ignoring specifications for indices.";

fn make_request(cmd: &AlterTableCmd) -> UpdateTableIn {
    UpdateTableIn {
        table_name: cmd.table.clone(),
        billing: cmd.billing.clone(),
        warm: cmd.warm.clone(),
        sse: cmd.sse.clone(),
        stream: cmd.stream.clone(),
        table_class: cmd.table_class,
        deletion_protection: cmd.deletion_protection,
        attribute_definitions: cmd.attribute_definitions.clone(),
        gsi_updates: cmd.gsi_updates.clone(),
        vector_index_updates: cmd.vector_index_updates.clone(),
        replica: None,
    }
}

fn validate_gsi_updates(cmd: &AlterTableCmd) -> Result<()> {
    for update in &cmd.gsi_updates {
        if update.action != GsiAction::Create {
            continue;
        }
        if let Some(ks) = &update.key_schema {
            if ks.hash.is_empty() {
                return fail(
                    Errc::InvalidArgument,
                    format!(
                        "GSI {} must specify a partition (HASH) key.",
                        update.index_name
                    ),
                );
            }
            if ks.hash.len() > 4 || ks.range.len() > 4 {
                return fail(
                    Errc::InvalidArgument,
                    format!(
                        "GSI {} key schema exceeds the 4-attribute limit per partition/sort key.",
                        update.index_name
                    ),
                );
            }
        }
    }
    Ok(())
}

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

pub fn execute_alter_table(cmd: &AlterTableCmd, client: &mut dyn DdbClient) -> Result<ExecOutcome> {
    validate_gsi_updates(cmd)?;
    client.update_table(&make_request(cmd))?;
    let mut status = StatusOutcome {
        token: "ALTER".into(),
        diagnostics: vec![],
    };
    if cmd.ignored_gsi_on_demand {
        status.diagnostics.push(ON_DEMAND_GSI_IGNORED.into());
    }
    Ok(ExecOutcome::Status(status))
}

pub fn execute_alter_table_explain(
    cmd: &AlterTableCmd,
    client: &mut dyn DdbClient,
) -> Result<ExecOutcome> {
    validate_gsi_updates(cmd)?;
    explain_one(
        client,
        "UpdateTable",
        ExplainRequest::UpdateTable(make_request(cmd)),
    )
}

pub fn execute_alter_ttl(
    cmd: &AlterTtlCmd,
    client: &mut dyn DdbClient,
    explain: bool,
) -> Result<ExecOutcome> {
    let probe = client.describe_time_to_live(&DescribeTimeToLiveIn {
        table_name: cmd.table.clone(),
    })?;
    if probe.status == "DISABLED" && !cmd.enabled {
        return Ok(ExecOutcome::status("ALTER"));
    }
    let request = UpdateTimeToLiveIn {
        table_name: cmd.table.clone(),
        enabled: cmd.enabled,
        attribute_name: if cmd.enabled {
            cmd.attribute.clone()
        } else {
            probe.attribute_name
        },
    };
    if explain {
        return explain_one(
            client,
            "UpdateTimeToLive",
            ExplainRequest::UpdateTimeToLive(request),
        );
    }
    client.update_time_to_live(&request)?;
    Ok(ExecOutcome::status("ALTER"))
}

pub fn execute_alter_pitr(
    cmd: &AlterPitrCmd,
    client: &mut dyn DdbClient,
    explain: bool,
) -> Result<ExecOutcome> {
    let request = UpdateContinuousBackupsIn {
        table_name: cmd.table.clone(),
        enabled: cmd.enabled,
    };
    if explain {
        return explain_one(
            client,
            "UpdateContinuousBackupsRequest",
            ExplainRequest::UpdateContinuousBackups(request),
        );
    }
    client.update_continuous_backups(&request)?;
    Ok(ExecOutcome::status("ALTER"))
}

pub fn execute_alter_replica(
    cmd: &AlterReplicaCmd,
    client: &mut dyn DdbClient,
    explain: bool,
) -> Result<ExecOutcome> {
    let request = UpdateTableIn {
        table_name: cmd.table.clone(),
        replica: Some(cmd.replica.clone()),
        ..Default::default()
    };
    if explain {
        return explain_one(client, "UpdateTable", ExplainRequest::UpdateTable(request));
    }
    client.update_table(&request)?;
    Ok(ExecOutcome::status("ALTER"))
}
