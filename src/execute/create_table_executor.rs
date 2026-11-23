//! CREATE TABLE (port of src/execute/create_table_executor.cpp).
use crate::client::{CreateTableIn, DdbClient, DescribeTableIn, ExplainRequest};
use crate::error::{fail, Errc, Result};
use crate::outcome::{ExecOutcome, ExplainBlock, ExplainFormat, ExplainOutcome};
use crate::parser::ast::{CreateTableCmd, KeySchema, ProjectionType};

fn is_single_key(ks: &KeySchema) -> bool {
    ks.hash.len() <= 1 && ks.range.len() <= 1
}

fn validate_key_schemas(cmd: &CreateTableCmd) -> Result<()> {
    if cmd.key_schema.hash.is_empty() {
        return fail(
            Errc::InvalidArgument,
            "A table primary key must specify a partition (HASH) key.",
        );
    }
    if !is_single_key(&cmd.key_schema) {
        return fail(
            Errc::InvalidArgument,
            "Composite keys are supported only on GSIs. A table primary key must have one partition key and an optional sort key.",
        );
    }
    for lsi in &cmd.lsis {
        if lsi.key_schema.hash.is_empty() {
            return fail(
                Errc::InvalidArgument,
                format!("LSI {} must specify a partition (HASH) key.", lsi.name),
            );
        }
        if !is_single_key(&lsi.key_schema) {
            return fail(
                Errc::InvalidArgument,
                format!(
                    "Composite keys are supported only on GSIs. LSI {} must have one partition key and an optional sort key.",
                    lsi.name
                ),
            );
        }
    }
    for gsi in &cmd.gsis {
        if gsi.key_schema.hash.is_empty() {
            return fail(
                Errc::InvalidArgument,
                format!("GSI {} must specify a partition (HASH) key.", gsi.name),
            );
        }
        if gsi.key_schema.hash.len() > 4 || gsi.key_schema.range.len() > 4 {
            return fail(
                Errc::InvalidArgument,
                format!(
                    "GSI {} key schema exceeds the 4-attribute limit per partition/sort key.",
                    gsi.name
                ),
            );
        }
    }
    let is_declared = |name: &str| cmd.attribute_definitions.iter().any(|a| a.name == name);
    for vi in &cmd.vector_indexes {
        if is_declared(&vi.vector_attr) {
            return fail(
                Errc::InvalidArgument,
                format!(
                    "The vector attribute {} of vector index {} must not be declared in the attribute list (it is a vector, not a scalar key).",
                    vi.vector_attr, vi.index_name
                ),
            );
        }
        for e in &vi.search_schema {
            if !is_declared(&e.attr) {
                return fail(
                    Errc::InvalidArgument,
                    format!(
                        "The search-key attribute {} of vector index {} must be declared in the attribute list.",
                        e.attr, vi.index_name
                    ),
                );
            }
        }
        if vi.projection.ty == ProjectionType::Include {
            for a in &vi.projection.non_key_attributes {
                if !is_declared(a) {
                    return fail(
                        Errc::InvalidArgument,
                        format!(
                            "The projected attribute {} of vector index {} must be declared in the attribute list.",
                            a, vi.index_name
                        ),
                    );
                }
            }
        }
    }
    Ok(())
}

fn make_request(cmd: &CreateTableCmd) -> CreateTableIn {
    CreateTableIn {
        table_name: cmd.table_name.clone(),
        attribute_definitions: cmd.attribute_definitions.clone(),
        key_schema: cmd.key_schema.clone(),
        billing: cmd.billing.clone(),
        warm: cmd.warm.clone(),
        gsis: cmd.gsis.clone(),
        lsis: cmd.lsis.clone(),
        vector_indexes: cmd.vector_indexes.clone(),
        stream: cmd.stream.clone(),
        sse: cmd.sse.clone(),
        table_class: cmd.table_class,
        deletion_protection: cmd.deletion_protection,
        tags: cmd.tags.clone(),
    }
}

pub fn execute_create_table(
    cmd: &CreateTableCmd,
    client: &mut dyn DdbClient,
) -> Result<ExecOutcome> {
    validate_key_schemas(cmd)?;
    if cmd.if_not_exists
        && client
            .describe_table(&DescribeTableIn {
                table_name: cmd.table_name.clone(),
            })
            .is_ok()
    {
        return Ok(ExecOutcome::status("CREATE"));
    }
    client.create_table(&make_request(cmd))?;
    if !cmd.nowait {
        client.wait_for_table_active(&cmd.table_name)?;
    }
    Ok(ExecOutcome::status("CREATE"))
}

pub fn execute_create_table_explain(
    cmd: &CreateTableCmd,
    client: &mut dyn DdbClient,
) -> Result<ExecOutcome> {
    validate_key_schemas(cmd)?;
    let mut outcome = ExplainOutcome {
        blocks: vec![],
        trailing_status: Some("CREATE".into()),
    };
    if cmd.if_not_exists
        && client
            .describe_table(&DescribeTableIn {
                table_name: cmd.table_name.clone(),
            })
            .is_ok()
    {
        return Ok(ExecOutcome::Explain(outcome));
    }
    let body = client.explain_request(&ExplainRequest::CreateTable(make_request(cmd)))?;
    outcome.blocks.push(ExplainBlock {
        api_name: "CreateTable".into(),
        body,
        format: ExplainFormat::Pretty,
    });
    Ok(ExecOutcome::Explain(outcome))
}
