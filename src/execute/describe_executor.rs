//! DESCRIBE <table> (port of src/execute/describe_executor.cpp).
use crate::client::{
    DdbClient, DescribeContinuousBackupsIn, DescribeTableIn, DescribeTableReplicaAutoScalingIn,
    DescribeTimeToLiveIn, ExplainRequest, TableKeySchema,
};
use crate::error::Result;
use crate::outcome::{ExecOutcome, ExplainBlock, ExplainFormat, ExplainOutcome, ReportOutcome};
use crate::parser::ast::{
    DescribeCmd, Projection, ProjectionType, ScalarType, SearchSchemaElem, SearchSchemaKind,
    VectorDistance,
};

pub(crate) fn scalar_type_name(t: ScalarType) -> &'static str {
    match t {
        ScalarType::N => "N",
        ScalarType::B => "B",
        ScalarType::S => "S",
    }
}

pub(crate) fn projection_type_name(t: ProjectionType) -> &'static str {
    match t {
        ProjectionType::KeysOnly => "KEYS_ONLY",
        ProjectionType::Include => "INCLUDE",
        ProjectionType::All => "ALL",
    }
}

pub(crate) fn vector_distance_name(d: VectorDistance) -> &'static str {
    match d {
        VectorDistance::Euclidean => "EUCLIDEAN",
        VectorDistance::DotProduct => "DOT_PRODUCT",
        VectorDistance::Cosine => "COSINE",
    }
}

fn vector_search_schema_text(schema: &[SearchSchemaElem]) -> String {
    schema
        .iter()
        .map(|e| {
            let kind = if e.kind == SearchSchemaKind::Hash {
                "HASH"
            } else {
                "INLINE_FILTER"
            };
            format!("{} ({})", e.attr, kind)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn key_schema_text(ks: &TableKeySchema) -> String {
    ks.hash_keys
        .iter()
        .map(|h| format!("HASH {}", h))
        .chain(ks.range_keys.iter().map(|r| format!("RANGE {}", r)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn projection_text(p: &Projection) -> String {
    if p.ty == ProjectionType::Include {
        format!("Projecting (INCLUDE) ({})", p.non_key_attributes.join(", "))
    } else {
        format!("Projecting ({})", projection_type_name(p.ty))
    }
}

pub fn execute_describe(cmd: &DescribeCmd, client: &mut dyn DdbClient) -> Result<ExecOutcome> {
    let td = client.describe_table(&DescribeTableIn {
        table_name: cmd.table.clone(),
    })?;
    let _ = client.describe_time_to_live(&DescribeTimeToLiveIn {
        table_name: cmd.table.clone(),
    });
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("Name: {} ({})", td.table_name, td.table_status));
    lines.push(format!("Key: {}", key_schema_text(&td.key_schema)));
    {
        let mut attrs = String::from("Attributes: ");
        for (i, a) in td.attribute_definitions.iter().enumerate() {
            if i > 0 {
                attrs += ", ";
            }
            attrs += &format!(" {}, {}", a.name, scalar_type_name(a.ty));
        }
        lines.push(attrs);
    }
    lines.push(format!("Created at: {}", td.creation_date_time));
    lines.push(format!("Table ARN: {}", td.table_arn));
    lines.push(format!("Table ID: {}", td.table_id));
    lines.push(format!("Table size (bytes): {}", td.table_size_bytes));
    lines.push(format!("Item Count: {}", td.item_count));
    lines.push(format!(
        "Deletion Protection: {}",
        if td.deletion_protection_enabled {
            "Enabled"
        } else {
            "Disabled"
        }
    ));
    let mut table_is_on_demand = false;
    let provisioned_line = format!(
        "Billing Mode: Provisioned ({} RCU, {} WCU)",
        td.provisioned_read_capacity_units, td.provisioned_write_capacity_units
    );
    if let Some(mode) = &td.billing_mode_summary {
        if mode == "PROVISIONED" {
            lines.push(provisioned_line);
        } else if mode == "PAY_PER_REQUEST" {
            lines.push("Billing Mode: On Demand".into());
            table_is_on_demand = true;
            if td.on_demand_throughput_set {
                lines.push(format!(
                    "On Demand Throughput: ({} RCU, {} WCU)",
                    td.on_demand_max_read_request_units, td.on_demand_max_write_request_units
                ));
            }
        } else {
            lines.push(format!("Unknown Billing mode {}.", mode));
        }
    } else if td.provisioned_throughput_set {
        lines.push(provisioned_line);
    }
    if let Some(wt) = &td.warm_throughput {
        lines.push(format!(
            "Warm Throughput: ({} RCU, {} WCU, Status {})",
            wt.read_units_per_second, wt.write_units_per_second, wt.status
        ));
    }
    if let Ok(cb) = client.describe_continuous_backups(&DescribeContinuousBackupsIn {
        table_name: cmd.table.clone(),
    }) {
        if cb.pitr_status == "ENABLED" {
            lines.push(format!(
                "PITR is Enabled: [{} to {}]",
                cb.earliest_restorable_date_time, cb.latest_restorable_date_time
            ));
        } else if cb.pitr_status == "DISABLED" {
            lines.push("PITR is Disabled.".into());
        }
    }
    if !td.global_secondary_indexes.is_empty() {
        for gsi in &td.global_secondary_indexes {
            let mut line = format!(
                "GSI {}: ( {} ), ",
                gsi.index_name,
                key_schema_text(&gsi.key_schema)
            );
            if !table_is_on_demand {
                line += &format!(
                    " Provisioned ({} RCU, {} WCU), ",
                    gsi.provisioned_read_capacity_units, gsi.provisioned_write_capacity_units
                );
            } else {
                line += "Billing Mode: On Demand (mirrors table), ";
                if gsi.on_demand_throughput_set {
                    line += &format!(
                        "Throughput ({} RCU, {} WCU), ",
                        gsi.on_demand_max_read_request_units, gsi.on_demand_max_write_request_units
                    );
                }
            }
            if gsi.projection_set {
                line += &projection_text(&gsi.projection);
            }
            line += &format!(", Status: {}", gsi.index_status);
            line += &format!(
                ", Backfilling: {}",
                if gsi.backfilling { "YES" } else { "NO" }
            );
            if let Some(w) = &gsi.warm_throughput {
                line += &format!(
                    ", Warm Throughput: ({} RCU, {} WCU, Status {})",
                    w.read_units_per_second, w.write_units_per_second, w.status
                );
            }
            lines.push(line);
        }
    } else {
        lines.push("GSI: None".into());
    }
    if !td.local_secondary_indexes.is_empty() {
        let mut line = String::new();
        for lsi in &td.local_secondary_indexes {
            line += &format!(
                "LSI {}: ( {} ), ",
                lsi.index_name,
                key_schema_text(&lsi.key_schema)
            );
        }
        lines.push(line);
    } else {
        lines.push("LSI: None".into());
    }
    if !td.vector_indexes.is_empty() {
        for vi in &td.vector_indexes {
            let mut line = format!(
                "VSI {}: Vector Attribute: {}, Dimensions: {}, Distance: {}",
                vi.index_name,
                vi.vector_attr,
                vi.dimensions,
                vector_distance_name(vi.distance)
            );
            let schema = vector_search_schema_text(&vi.search_schema);
            if !schema.is_empty() {
                line += &format!(", Search Schema: ( {} )", schema);
            }
            if vi.projection_set {
                line += ", ";
                line += &projection_text(&vi.projection);
            }
            line += &format!(", Status: {}", vi.index_status);
            lines.push(line);
        }
    } else {
        lines.push("VSI: None".into());
    }
    match &td.stream_view_type {
        Some(s) => lines.push(format!("Stream: {}", s)),
        None => lines.push("Stream: Disabled".into()),
    }
    match &td.table_class_summary {
        Some(c) => lines.push(format!("Table Class: {}", c)),
        None => lines.push("Table Class: Has not been set.".into()),
    }
    match &td.sse_status {
        Some(s) => lines.push(format!("SSE: ({})", s)),
        None => lines.push("SSE: Not set".into()),
    }
    if let Ok(scaling) =
        client.describe_table_replica_auto_scaling(&DescribeTableReplicaAutoScalingIn {
            table_name: cmd.table.clone(),
        })
    {
        for r in &scaling.replicas {
            lines.push(format!(
                "Replica Region: {} (Status: {})",
                r.region_name, r.replica_status
            ));
        }
    }
    Ok(ExecOutcome::Report(ReportOutcome { lines }))
}

pub fn execute_describe_explain(
    cmd: &DescribeCmd,
    client: &mut dyn DdbClient,
) -> Result<ExecOutcome> {
    let t = cmd.table.clone();
    let specs = [
        (
            "DescribeTable",
            ExplainRequest::DescribeTable(DescribeTableIn {
                table_name: t.clone(),
            }),
            ExplainFormat::Compact,
        ),
        (
            "DescribeTimeToLive",
            ExplainRequest::DescribeTimeToLive(DescribeTimeToLiveIn {
                table_name: t.clone(),
            }),
            ExplainFormat::Compact,
        ),
        (
            "DescribeContinuousBackups",
            ExplainRequest::DescribeContinuousBackups(DescribeContinuousBackupsIn {
                table_name: t.clone(),
            }),
            ExplainFormat::Pretty,
        ),
        (
            "DescribeTableReplicaAutoScaling",
            ExplainRequest::DescribeTableReplicaAutoScaling(DescribeTableReplicaAutoScalingIn {
                table_name: t,
            }),
            ExplainFormat::Pretty,
        ),
    ];
    let mut outcome = ExplainOutcome::default();
    for (api, req, format) in specs {
        let body = client.explain_request(&req)?;
        outcome.blocks.push(ExplainBlock {
            api_name: api.into(),
            body,
            format,
        });
    }
    Ok(ExecOutcome::Explain(outcome))
}
