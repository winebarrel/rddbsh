// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! DROP TABLE (port of src/execute/drop_table_executor.cpp).
use crate::client::{DdbClient, DeleteTableIn, DescribeTableIn, ExplainRequest};
use crate::error::Result;
use crate::outcome::{ExecOutcome, ExplainBlock, ExplainFormat, ExplainOutcome};
use crate::parser::ast::DropTableCmd;

pub fn execute_drop_table(cmd: &DropTableCmd, client: &mut dyn DdbClient) -> Result<ExecOutcome> {
    if cmd.if_exists
        && client
            .describe_table(&DescribeTableIn {
                table_name: cmd.table.clone(),
            })
            .is_err()
    {
        return Ok(ExecOutcome::Empty);
    }
    client.delete_table(&DeleteTableIn {
        table_name: cmd.table.clone(),
    })?;
    if !cmd.nowait {
        client.wait_for_table_gone(&cmd.table)?;
    }
    Ok(ExecOutcome::status("DROP"))
}

pub fn execute_drop_table_explain(
    cmd: &DropTableCmd,
    client: &mut dyn DdbClient,
) -> Result<ExecOutcome> {
    if cmd.if_exists
        && client
            .describe_table(&DescribeTableIn {
                table_name: cmd.table.clone(),
            })
            .is_err()
    {
        return Ok(ExecOutcome::Empty);
    }
    let body = client.explain_request(&ExplainRequest::DeleteTable(DeleteTableIn {
        table_name: cmd.table.clone(),
    }))?;
    Ok(ExecOutcome::Explain(ExplainOutcome {
        blocks: vec![ExplainBlock {
            api_name: "DeleteTable".into(),
            body,
            format: ExplainFormat::Pretty,
        }],
        trailing_status: None,
    }))
}
