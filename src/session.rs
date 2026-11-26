// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! The session: parse + dispatch to executors (port of src/session.cpp).

use crate::client::DdbClient;
use crate::error::{fail, Errc, Error, Result};
use crate::execute;
use crate::expressions::unique_prefix;
use crate::outcome::{
    BufferingSink, ExecOutcome, ExplainOutcome, OutcomeSink, ReportOutcome, ReportTable,
};
use crate::parser::{self, ast::CommandNode, help_text};
use crate::statement::{Statement, StatementKind};
use crate::version;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionConfig {
    pub region: String,
    pub endpoint: String,
}

#[derive(Debug, Default)]
pub struct ParsedBatch {
    pub statements: Vec<Statement>,
    pub error: Option<Error>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConnectionHint {
    pub region: String,
    pub endpoint_overridden: bool,
    /// the endpoint override (empty when none)
    pub endpoint: String,
}

pub struct Session {
    config: SessionConfig,
    client: Box<dyn DdbClient>,
    alias_prefix_counter: u64,
}

/// Fold the fragments of a command that streams per response back into one
/// outcome. None for every other command.
///   * SHOW TABLES / BACKUPS / LIMITS, DESCRIBE: every report's lines in one
///     report, and every fragment's table rows in one table.
///   * bulk UPDATE / DELETE under EXPLAIN: every block in one ExplainOutcome.
///   * bulk UPDATE / DELETE with RETURN VALUES: the partial fragments' items
///     ahead of the final fragment's, in that one final BulkWriteOutcome.
fn fold_streamed(statement: &Statement, parts: &mut Vec<ExecOutcome>) -> Option<ExecOutcome> {
    match &statement.command {
        CommandNode::ShowTables(_)
        | CommandNode::ShowBackups(_)
        | CommandNode::ShowLimits
        | CommandNode::Describe(_) => {
            if statement.explain {
                return None;
            }
            let mut report = ReportOutcome::default();
            for part in parts.drain(..) {
                if let ExecOutcome::Report(r) = part {
                    report.lines.extend(r.lines);
                    if let Some(t) = r.table {
                        let folded = report.table.get_or_insert_with(|| ReportTable {
                            columns: t.columns.clone(),
                            rows: vec![],
                            group_end: true,
                        });
                        folded.rows.extend(t.rows);
                    }
                }
            }
            Some(ExecOutcome::Report(report))
        }
        CommandNode::Update(_) | CommandNode::Delete(_) => {
            if parts.len() < 2 {
                return None;
            }
            if matches!(parts[0], ExecOutcome::Explain(_)) {
                let mut merged = ExplainOutcome::default();
                for part in parts.iter() {
                    let ExecOutcome::Explain(e) = part else {
                        return None;
                    };
                    merged.blocks.extend(e.blocks.iter().cloned());
                    if e.trailing_status.is_some() {
                        merged.trailing_status = e.trailing_status.clone();
                    }
                }
                return Some(ExecOutcome::Explain(merged));
            }
            let mut items = Vec::new();
            for part in parts.iter() {
                let ExecOutcome::BulkWrite(b) = part else {
                    return None;
                };
                items.extend(b.returned_items.iter().cloned());
            }
            let ExecOutcome::BulkWrite(mut last) = parts.pop()? else {
                return None;
            };
            last.returned_items = items;
            last.partial = false;
            Some(ExecOutcome::BulkWrite(last))
        }
        _ => None,
    }
}

const UNBOUND: &str = "statement has unbound parameter marker(s); use prepare() and bind()";

impl Session {
    pub fn new(config: SessionConfig, client: Box<dyn DdbClient>) -> Session {
        Session {
            config,
            client,
            alias_prefix_counter: 0,
        }
    }

    pub fn client(&mut self) -> &mut dyn DdbClient {
        &mut *self.client
    }

    pub fn next_alias_prefix(&mut self) -> String {
        let p = unique_prefix(self.alias_prefix_counter);
        self.alias_prefix_counter += 1;
        p
    }

    /// Parse exactly one statement.
    pub fn parse(&mut self, text: &str) -> Result<Statement> {
        parser::parse_one(text)
    }

    /// Parse every `;`-terminated statement in order (missing trailing `;` tolerated).
    pub fn parse_batch(&mut self, text: &str) -> ParsedBatch {
        let (statements, error) = parser::parse_batch(text);
        ParsedBatch { statements, error }
    }

    /// Execute and collect every fragment into one outcome.
    pub fn execute(&mut self, statement: &Statement) -> Result<ExecOutcome> {
        let mut sink = BufferingSink::default();
        self.execute_streaming(statement, &mut sink)?;
        // The commands that stream per response return the single outcome
        // they returned before they streamed.
        if let Some(folded) = fold_streamed(statement, &mut sink.parts) {
            return Ok(folded);
        }
        Ok(match sink.parts.len() {
            0 => ExecOutcome::Empty,
            1 => sink.parts.pop().unwrap(),
            _ => ExecOutcome::Multi(sink.parts),
        })
    }

    /// Execute, emitting outcome fragments to `sink` as they are produced.
    /// Every call ends with exactly one `sink.end_statement()`, after the last
    /// emit, on success and on failure, so a sink holding an open result set
    /// closes it before the caller reports an error.
    pub fn execute_streaming(
        &mut self,
        statement: &Statement,
        sink: &mut dyn OutcomeSink,
    ) -> Result<()> {
        let result = self.dispatch(statement, sink);
        sink.end_statement();
        result
    }

    fn dispatch(&mut self, statement: &Statement, sink: &mut dyn OutcomeSink) -> Result<()> {
        if parser::count_param_markers(statement) > 0 {
            return fail(Errc::UnboundParameter, UNBOUND);
        }
        let explain = statement.explain;
        let counter = &mut self.alias_prefix_counter;
        let mut prefix_gen = move || {
            let p = unique_prefix(*counter);
            *counter += 1;
            p
        };
        let client: &mut dyn DdbClient = &mut *self.client;
        let mut emit = |r: Result<ExecOutcome>| -> Result<()> {
            sink.emit(r?);
            Ok(())
        };
        match &statement.command {
            CommandNode::Empty => emit(Ok(ExecOutcome::Empty)),
            CommandNode::ShowVersion => emit(Ok(ExecOutcome::Report(ReportOutcome {
                lines: vec![format!("ddbsh - version {}", version())],
                table: None,
            }))),
            CommandNode::ShowConnection => {
                let region = &self.config.region;
                let endpoint = &self.config.endpoint;
                let lines = vec![
                    format!("ddbsh - version {}", version()),
                    format!(
                        "Region: {}",
                        if region.is_empty() {
                            "(default)"
                        } else {
                            region
                        }
                    ),
                    format!(
                        "Endpoint: {}",
                        if endpoint.is_empty() {
                            "(default)"
                        } else {
                            endpoint
                        }
                    ),
                    format!("Backend: {}", client.connection_descriptor()),
                ];
                emit(Ok(ExecOutcome::Report(ReportOutcome {
                    lines,
                    table: None,
                })))
            }
            CommandNode::Quit => emit(Ok(ExecOutcome::status("QUIT"))),
            CommandNode::Help(help) => emit(Ok(ExecOutcome::Report(ReportOutcome {
                lines: help_text::help_lines(help_text::help_text(help.topic)),
                table: None,
            }))),
            CommandNode::Connect(c) => {
                self.config.region = c.region.clone();
                self.config.endpoint = c.endpoint.clone();
                let _ = client.connect(&c.region, &c.endpoint);
                emit(Ok(ExecOutcome::status("CONNECT")))
            }
            CommandNode::Select(select) => {
                if explain {
                    emit(execute::select_executor::execute_select_explain(
                        select,
                        client,
                        &prefix_gen(),
                    ))
                } else {
                    execute::select_executor::execute_select_streaming(
                        select,
                        client,
                        &prefix_gen(),
                        sink,
                    )
                }
            }
            CommandNode::DropTable(drop) => {
                if explain {
                    emit(execute::drop_table_executor::execute_drop_table_explain(
                        drop, client,
                    ))
                } else {
                    emit(execute::drop_table_executor::execute_drop_table(
                        drop, client,
                    ))
                }
            }
            CommandNode::Insert(insert) => {
                if explain {
                    emit(execute::insert_executor::execute_insert_explain(
                        insert,
                        client,
                        &mut prefix_gen,
                    ))
                } else {
                    execute::insert_executor::execute_insert_streaming(
                        insert,
                        client,
                        &mut prefix_gen,
                        sink,
                    )
                }
            }
            CommandNode::Update(update) => execute::update_executor::execute_update(
                update,
                client,
                &mut prefix_gen,
                explain,
                sink,
            ),
            CommandNode::Delete(del) => execute::delete_executor::execute_delete(
                del,
                client,
                &mut prefix_gen,
                explain,
                sink,
            ),
            CommandNode::Transact(txn) => execute::transact_executor::execute_transact(
                txn,
                client,
                &mut prefix_gen,
                explain,
                sink,
            ),
            CommandNode::CreateTable(create) => {
                if explain {
                    emit(
                        execute::create_table_executor::execute_create_table_explain(
                            create, client,
                        ),
                    )
                } else {
                    emit(execute::create_table_executor::execute_create_table(
                        create, client,
                    ))
                }
            }
            CommandNode::AlterTable(alter) => {
                if explain {
                    emit(execute::alter_table_executor::execute_alter_table_explain(
                        alter, client,
                    ))
                } else {
                    emit(execute::alter_table_executor::execute_alter_table(
                        alter, client,
                    ))
                }
            }
            CommandNode::AlterTtl(ttl) => emit(execute::alter_table_executor::execute_alter_ttl(
                ttl, client, explain,
            )),
            CommandNode::AlterPitr(pitr) => emit(
                execute::alter_table_executor::execute_alter_pitr(pitr, client, explain),
            ),
            CommandNode::AlterReplica(replica) => emit(
                execute::alter_table_executor::execute_alter_replica(replica, client, explain),
            ),
            CommandNode::ShowTables(show) => {
                execute::show_tables_executor::execute_show_tables(show, client, sink)
            }
            CommandNode::Describe(d) => {
                if explain {
                    emit(execute::describe_executor::execute_describe_explain(
                        d, client,
                    ))
                } else {
                    execute::describe_executor::execute_describe(d, client, sink)
                }
            }
            CommandNode::CreateBackup(c) => emit(
                execute::backup_restore_executor::execute_create_backup(c, client, explain),
            ),
            CommandNode::DropBackup(c) => emit(
                execute::backup_restore_executor::execute_drop_backup(c, client, explain),
            ),
            CommandNode::DescribeBackup(c) => emit(
                execute::backup_restore_executor::execute_describe_backup(c, client, explain),
            ),
            CommandNode::Restore(c) => emit(execute::backup_restore_executor::execute_restore(
                c, client, explain,
            )),
            CommandNode::ShowBackups(c) => {
                execute::show_commands_executor::execute_show_backups(c, client, sink)
            }
            CommandNode::ShowLimits => {
                let region = self.config.region.clone();
                execute::show_commands_executor::execute_show_limits(client, &region, sink)
            }
            CommandNode::ShowCreateTable(c) => emit(
                execute::show_commands_executor::execute_show_create_table(c, client),
            ),
        }
    }

    /// Parse one statement and execute it.
    pub fn run(&mut self, text: &str) -> Result<ExecOutcome> {
        let stmt = self.parse(text)?;
        self.execute(&stmt)
    }

    pub fn connection_hint(&self) -> ConnectionHint {
        ConnectionHint {
            region: self.config.region.clone(),
            endpoint_overridden: !self.config.endpoint.is_empty(),
            endpoint: self.config.endpoint.clone(),
        }
    }

    pub fn statement_kind(statement: &Statement) -> StatementKind {
        statement.kind()
    }
}
