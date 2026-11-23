// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! Recursive-descent parser (port of src/parser/grammar.yy + driver.cpp).
//!
//! Accepts exactly the bison grammar's language. One statement is parsed per
//! step; the token cursor (and hence the `[line, col]` of a later error) is
//! continuous across the statements of a batch. Errors are reported at the
//! first token that cannot extend a valid statement — the same token the LALR
//! parser rejects — composed as `[line, col] syntax error at <lexeme>` plus a
//! caret diagnostic, exactly as driver.cpp does.

use super::ast::*;
use super::lexer::{tokenize, Kw, Tok, Token};
use crate::error::{Errc, Error, Result};
use crate::expressions::{LogicalExpr, Where};
use crate::value::{Value, ValueList};

/// Parse exactly one statement (trailing `;` optional). Empty or comment-only
/// input yields the empty statement.
pub fn parse_one(text: &str) -> Result<ParsedStatement> {
    let mut driver = Driver::new(text);
    match driver.parse_next() {
        Step::Statement(s) => Ok(*s),
        Step::EmptyAtEof => Ok(ParsedStatement::default()),
        Step::Error(e) => Err(e),
    }
}

/// Parse every statement in the batch, stopping at the first error.
pub fn parse_batch(text: &str) -> (Vec<ParsedStatement>, Option<Error>) {
    let mut driver = Driver::new(text);
    let mut statements = Vec::new();
    loop {
        match driver.parse_next() {
            Step::Statement(s) => statements.push(*s),
            Step::EmptyAtEof => return (statements, None),
            Step::Error(e) => return (statements, Some(e)),
        }
    }
}

enum Step {
    Statement(Box<ParsedStatement>),
    EmptyAtEof,
    Error(Error),
}

/// Marker for "a syntax error was recorded on the driver".
struct Syntax;
type P<T> = std::result::Result<T, Syntax>;

struct Driver<'a> {
    input: &'a str,
    tokens: Vec<Token>,
    pos: usize,
    saw_eof: bool,
    param_count: u32,
    error: Option<Error>,
}

fn is_write_start(t: &Tok) -> bool {
    matches!(
        t,
        Tok::Kw(
            Kw::Insert | Kw::Replace | Kw::Update | Kw::Upsert | Kw::Delete | Kw::Exists | Kw::Not
        )
    )
}

impl<'a> Driver<'a> {
    fn new(input: &'a str) -> Driver<'a> {
        Driver {
            input,
            tokens: tokenize(input),
            pos: 0,
            saw_eof: false,
            param_count: 0,
            error: None,
        }
    }

    // ---- token access -------------------------------------------------

    fn peek(&mut self) -> &Tok {
        let idx = self.pos.min(self.tokens.len() - 1);
        if self.tokens[idx].tok == Tok::End {
            self.saw_eof = true;
        }
        &self.tokens[idx].tok
    }

    fn advance(&mut self) -> Tok {
        let idx = self.pos.min(self.tokens.len() - 1);
        let t = self.tokens[idx].tok.clone();
        if t != Tok::End {
            self.pos += 1;
        }
        t
    }

    fn is_kw(&mut self, kw: Kw) -> bool {
        *self.peek() == Tok::Kw(kw)
    }

    fn is_char(&mut self, c: char) -> bool {
        *self.peek() == Tok::Char(c)
    }

    fn accept_kw(&mut self, kw: Kw) -> bool {
        if self.is_kw(kw) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn accept_char(&mut self, c: char) -> bool {
        if self.is_char(c) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect_kw(&mut self, kw: Kw) -> P<()> {
        if self.accept_kw(kw) {
            Ok(())
        } else {
            self.syntax_error()
        }
    }

    fn expect_char(&mut self, c: char) -> P<()> {
        if self.accept_char(c) {
            Ok(())
        } else {
            self.syntax_error()
        }
    }

    // ---- errors -------------------------------------------------------

    /// Record "[line, col] syntax error at <lexeme>" + caret for the current
    /// lookahead token (driver.cpp set_error; overrides any earlier error).
    fn syntax_error<T>(&mut self) -> P<T> {
        self.peek();
        let idx = self.pos.min(self.tokens.len() - 1);
        let tok = &self.tokens[idx];
        let mut composed = format!(
            "[{}, {}] syntax error at {}",
            tok.line, tok.column, tok.last_lexeme
        );
        let caret = build_caret(self.input, tok.line, tok.column, tok.last_lexeme.len());
        if !caret.is_empty() {
            composed.push('\n');
            composed.push_str(&caret);
        }
        self.error = Some(Error::new(Errc::ParseError, composed));
        Err(Syntax)
    }

    /// A verbatim semantic error (first one wins).
    fn semantic_error(&mut self, message: &str) {
        if self.error.is_none() {
            self.error = Some(Error::new(Errc::InvalidArgument, message));
        }
    }

    // ---- statement loop -------------------------------------------------

    fn parse_next(&mut self) -> Step {
        self.error = None;
        self.param_count = 0;
        let result = self.input_rule();
        match (result, self.error.take()) {
            (_, Some(e)) => Step::Error(e),
            (Err(Syntax), None) => Step::Error(Error::new(Errc::ParseError, "parse error")),
            (Ok(stmt), None) => {
                if stmt.command == CommandNode::Empty && self.saw_eof {
                    Step::EmptyAtEof
                } else {
                    Step::Statement(Box::new(stmt))
                }
            }
        }
    }

    /// input: %empty | ';' | command [';'] | EXPLAIN explainable [';']
    fn input_rule(&mut self) -> P<ParsedStatement> {
        match self.peek().clone() {
            Tok::End => Ok(ParsedStatement::default()),
            Tok::Semicolon => {
                self.advance();
                Ok(ParsedStatement::default())
            }
            Tok::Kw(Kw::Explain) => {
                self.advance();
                let command = self.explainable()?;
                self.statement_end()?;
                Ok(ParsedStatement {
                    command,
                    explain: true,
                })
            }
            _ => {
                let command = self.command()?;
                self.statement_end()?;
                Ok(ParsedStatement {
                    command,
                    explain: false,
                })
            }
        }
    }

    fn statement_end(&mut self) -> P<()> {
        match self.peek() {
            Tok::Semicolon => {
                self.advance();
                Ok(())
            }
            Tok::End => Ok(()),
            _ => self.syntax_error(),
        }
    }

    fn command(&mut self) -> P<CommandNode> {
        match self.peek().clone() {
            Tok::Kw(Kw::Connect) => self.connect_command(),
            Tok::Kw(Kw::Quit) => {
                self.advance();
                Ok(CommandNode::Quit)
            }
            Tok::Kw(Kw::Show) => self.show_command(),
            Tok::Kw(Kw::Help) => self.help_command(),
            _ => self.explainable(),
        }
    }

    /// explainable: ddl_command | dml_command | transact_command
    fn explainable(&mut self) -> P<CommandNode> {
        match self.peek().clone() {
            Tok::Kw(Kw::Drop) => self.drop_command(),
            Tok::Kw(Kw::Describe) => self.describe_command(),
            Tok::Kw(Kw::Create) => self.create_table_command(),
            Tok::Kw(Kw::Alter) => self.alter_table_command(),
            Tok::Kw(Kw::Backup) => self.create_backup_command(),
            Tok::Kw(Kw::Restore) => self.restore_command(),
            Tok::Kw(Kw::Select) => self.select_command(),
            Tok::Kw(Kw::Begin) => self.transact_command(),
            t if is_write_start(&t) => self.write_command(),
            _ => self.syntax_error(),
        }
    }

    // ---- control / show / help ------------------------------------------

    fn string(&mut self) -> P<String> {
        match self.peek().clone() {
            Tok::DQuoted(s) => {
                self.advance();
                Ok(unquote(&s))
            }
            Tok::Unquoted(s) => {
                self.advance();
                Ok(s)
            }
            _ => self.syntax_error(),
        }
    }

    fn is_string(&mut self) -> bool {
        matches!(self.peek(), Tok::DQuoted(_) | Tok::Unquoted(_))
    }

    fn whole(&mut self) -> P<String> {
        match self.peek().clone() {
            Tok::Whole(s) => {
                self.advance();
                Ok(s)
            }
            _ => self.syntax_error(),
        }
    }

    fn connect_command(&mut self) -> P<CommandNode> {
        self.expect_kw(Kw::Connect)?;
        let region = self.string()?;
        let mut endpoint = String::new();
        if self.accept_kw(Kw::With) {
            self.expect_kw(Kw::Endpoint)?;
            endpoint = self.string()?;
        }
        Ok(CommandNode::Connect(ConnectCmd { region, endpoint }))
    }

    fn show_command(&mut self) -> P<CommandNode> {
        self.expect_kw(Kw::Show)?;
        match self.peek().clone() {
            Tok::Kw(Kw::Tables) => {
                self.advance();
                let brief = self.accept_kw(Kw::Brief);
                let like = if self.accept_kw(Kw::Like) {
                    self.string()?
                } else {
                    String::new()
                };
                Ok(CommandNode::ShowTables(ShowTablesCmd { brief, like }))
            }
            Tok::Kw(Kw::Version) => {
                self.advance();
                Ok(CommandNode::ShowVersion)
            }
            Tok::Kw(Kw::Connection) => {
                self.advance();
                Ok(CommandNode::ShowConnection)
            }
            Tok::Kw(Kw::Limits) => {
                self.advance();
                Ok(CommandNode::ShowLimits)
            }
            Tok::Kw(Kw::Create) => {
                self.advance();
                self.expect_kw(Kw::Table)?;
                let if_not_exists = self.if_not_exists_clause()?;
                let nowait = self.accept_kw(Kw::Nowait);
                let table = self.string()?;
                Ok(CommandNode::ShowCreateTable(ShowCreateTableCmd {
                    if_not_exists,
                    nowait,
                    table,
                }))
            }
            _ => {
                let backup_type = if self.is_string() {
                    self.string()?
                } else {
                    String::new()
                };
                self.expect_kw(Kw::Backups)?;
                let table = if self.is_string() {
                    self.string()?
                } else {
                    String::new()
                };
                Ok(CommandNode::ShowBackups(ShowBackupsCmd {
                    backup_type,
                    table,
                }))
            }
        }
    }

    fn if_not_exists_clause(&mut self) -> P<bool> {
        if self.accept_kw(Kw::If) {
            self.expect_kw(Kw::Not)?;
            self.expect_kw(Kw::Exists)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn help_command(&mut self) -> P<CommandNode> {
        self.expect_kw(Kw::Help)?;
        use HelpTopic as H;
        let topic = match self.peek().clone() {
            Tok::Kw(Kw::Explain) => {
                self.advance();
                H::Explain
            }
            Tok::Kw(Kw::Begin) => {
                self.advance();
                H::Begin
            }
            Tok::Kw(Kw::Commit) => {
                self.advance();
                H::Commit
            }
            Tok::Kw(Kw::Rollback) => {
                self.advance();
                H::Rollback
            }
            Tok::Kw(Kw::Connect) => {
                self.advance();
                H::Connect
            }
            Tok::Kw(Kw::Drop) => {
                self.advance();
                if self.accept_kw(Kw::Table) {
                    H::DropTable
                } else if self.accept_kw(Kw::Backup) {
                    H::DropBackup
                } else {
                    H::Drop
                }
            }
            Tok::Kw(Kw::Describe) => {
                self.advance();
                if self.accept_kw(Kw::Backup) {
                    H::DescribeBackup
                } else {
                    H::Describe
                }
            }
            Tok::Kw(Kw::Show) => {
                self.advance();
                if self.accept_kw(Kw::Tables) {
                    H::ShowTables
                } else if self.accept_kw(Kw::Limits) {
                    H::ShowLimits
                } else if self.accept_kw(Kw::Backups) {
                    H::ShowBackups
                } else if self.accept_kw(Kw::Version) {
                    H::ShowVersion
                } else if self.accept_kw(Kw::Create) {
                    self.expect_kw(Kw::Table)?;
                    H::ShowCreateTable
                } else {
                    H::Show
                }
            }
            Tok::Kw(Kw::Create) => {
                self.advance();
                if self.accept_kw(Kw::Table) {
                    H::CreateTable
                } else {
                    H::Create
                }
            }
            Tok::Kw(Kw::Alter) => {
                self.advance();
                self.expect_kw(Kw::Table)?;
                H::AlterTable
            }
            Tok::Kw(Kw::Backup) => {
                self.advance();
                H::Backup
            }
            Tok::Kw(Kw::Restore) => {
                self.advance();
                H::Restore
            }
            Tok::Kw(Kw::Select) => {
                self.advance();
                H::SelectStatement
            }
            Tok::Kw(Kw::Insert) => {
                self.advance();
                H::Insert
            }
            Tok::Kw(Kw::Replace) => {
                self.advance();
                H::Replace
            }
            Tok::Kw(Kw::Update) => {
                self.advance();
                H::Update
            }
            Tok::Kw(Kw::Upsert) => {
                self.advance();
                H::Upsert
            }
            Tok::Kw(Kw::Delete) => {
                self.advance();
                H::Del
            }
            _ => H::General,
        };
        Ok(CommandNode::Help(HelpCmd { topic }))
    }

    // ---- DDL ------------------------------------------------------------

    fn drop_command(&mut self) -> P<CommandNode> {
        self.expect_kw(Kw::Drop)?;
        if self.accept_kw(Kw::Backup) {
            let backup_arn = self.string()?;
            return Ok(CommandNode::DropBackup(DropBackupCmd { backup_arn }));
        }
        self.expect_kw(Kw::Table)?;
        let if_exists = if self.accept_kw(Kw::If) {
            self.expect_kw(Kw::Exists)?;
            true
        } else {
            false
        };
        let nowait = self.accept_kw(Kw::Nowait);
        let table = self.string()?;
        Ok(CommandNode::DropTable(DropTableCmd {
            table,
            if_exists,
            nowait,
        }))
    }

    fn describe_command(&mut self) -> P<CommandNode> {
        self.expect_kw(Kw::Describe)?;
        if self.accept_kw(Kw::Backup) {
            let backup_arn = self.string()?;
            return Ok(CommandNode::DescribeBackup(DescribeBackupCmd {
                backup_arn,
            }));
        }
        let table = self.string()?;
        Ok(CommandNode::Describe(DescribeCmd { table }))
    }

    fn create_backup_command(&mut self) -> P<CommandNode> {
        self.expect_kw(Kw::Backup)?;
        self.expect_kw(Kw::Table)?;
        let table = self.string()?;
        self.expect_kw(Kw::Called)?;
        let backup_name = self.string()?;
        Ok(CommandNode::CreateBackup(CreateBackupCmd {
            table,
            backup_name,
        }))
    }

    fn restore_command(&mut self) -> P<CommandNode> {
        self.expect_kw(Kw::Restore)?;
        self.expect_kw(Kw::Table)?;
        let target = self.string()?;
        self.expect_kw(Kw::From)?;
        if self.accept_kw(Kw::Backup) {
            let source = self.string()?;
            return Ok(CommandNode::Restore(RestoreCmd {
                kind: RestoreKind::FromBackup,
                target,
                source,
                timestamp: String::new(),
            }));
        }
        let source = self.string()?;
        self.expect_kw(Kw::To)?;
        self.expect_kw(Kw::Pitr)?;
        let timestamp = match self.peek().clone() {
            Tok::Timestamp(t) => {
                self.advance();
                unquote(&t)
            }
            _ => return self.syntax_error(),
        };
        Ok(CommandNode::Restore(RestoreCmd {
            kind: RestoreKind::ToPitr,
            target,
            source,
            timestamp,
        }))
    }

    fn attribute_type(&mut self) -> P<ScalarType> {
        match self.peek() {
            Tok::Kw(Kw::AttributeTypeNumber) => {
                self.advance();
                Ok(ScalarType::N)
            }
            Tok::Kw(Kw::AttributeTypeString) => {
                self.advance();
                Ok(ScalarType::S)
            }
            Tok::Kw(Kw::AttributeTypeBinary) => {
                self.advance();
                Ok(ScalarType::B)
            }
            _ => self.syntax_error(),
        }
    }

    /// attribute_name_type_list: string attribute_type [',' ...]
    fn attribute_name_type_list(&mut self) -> P<Vec<AttributeDef>> {
        let mut list = Vec::new();
        loop {
            let name = self.string()?;
            let ty = self.attribute_type()?;
            list.push(AttributeDef { name, ty });
            if !self.accept_char(',') {
                return Ok(list);
            }
        }
    }

    /// key_schema: '(' key_role [',' key_role]* ')'
    fn key_schema(&mut self) -> P<KeySchema> {
        self.expect_char('(')?;
        let mut ks = KeySchema::default();
        loop {
            let attrs = if self.accept_char('(') {
                let mut names = vec![self.string()?];
                while self.accept_char(',') {
                    names.push(self.string()?);
                }
                self.expect_char(')')?;
                names
            } else {
                vec![self.string()?]
            };
            let is_hash = if self.accept_kw(Kw::Hash) {
                true
            } else if self.accept_kw(Kw::Range) {
                false
            } else {
                return self.syntax_error();
            };
            add_key_role(&mut ks, is_hash, attrs);
            if !self.accept_char(',') {
                break;
            }
        }
        self.expect_char(')')?;
        Ok(ks)
    }

    /// `( N rcu|wcu , N wcu|rcu )` — returns (rcu, wcu).
    fn rcu_wcu_pair(&mut self) -> P<(i64, i64)> {
        self.expect_char('(')?;
        let a = parse_ll(&self.whole()?);
        let first_rcu = if self.accept_kw(Kw::Rcu) {
            true
        } else if self.accept_kw(Kw::Wcu) {
            false
        } else {
            return self.syntax_error();
        };
        self.expect_char(',')?;
        let b = parse_ll(&self.whole()?);
        self.expect_kw(if first_rcu { Kw::Wcu } else { Kw::Rcu })?;
        self.expect_char(')')?;
        Ok(if first_rcu { (a, b) } else { (b, a) })
    }

    fn billing_mode_and_throughput(&mut self) -> P<BillingModeAndThroughput> {
        self.expect_kw(Kw::Billing)?;
        self.expect_kw(Kw::Mode)?;
        let mut b = BillingModeAndThroughput::default();
        if self.accept_kw(Kw::Provisioned) {
            b.mode = BillingMode::Provisioned;
            let (rcu, wcu) = self.rcu_wcu_pair()?;
            b.rcu = rcu;
            b.wcu = wcu;
            return Ok(b);
        }
        self.expect_kw(Kw::On)?;
        self.expect_kw(Kw::Demand)?;
        b.mode = BillingMode::PayPerRequest;
        if self.accept_kw(Kw::Throughput) {
            if self.accept_kw(Kw::Unlimited) {
                b.od.max_read_request_units = Some(-1);
                b.od.max_write_request_units = Some(-1);
            } else {
                let (rcu, wcu) = self.rcu_wcu_pair()?;
                b.od.max_read_request_units = Some(rcu);
                b.od.max_write_request_units = Some(wcu);
            }
        }
        Ok(b)
    }

    fn optional_billing(&mut self) -> P<Option<BillingModeAndThroughput>> {
        if self.is_kw(Kw::Billing) {
            Ok(Some(self.billing_mode_and_throughput()?))
        } else {
            Ok(None)
        }
    }

    fn warm_throughput(&mut self) -> P<WarmThroughput> {
        self.expect_kw(Kw::Warm)?;
        self.expect_kw(Kw::Throughput)?;
        let (rcu, wcu) = self.rcu_wcu_pair()?;
        Ok(WarmThroughput {
            read_units_per_second: rcu,
            write_units_per_second: wcu,
        })
    }

    fn optional_warm(&mut self) -> P<Option<WarmThroughput>> {
        if self.is_kw(Kw::Warm) {
            Ok(Some(self.warm_throughput()?))
        } else {
            Ok(None)
        }
    }

    fn index_projection(&mut self) -> P<Projection> {
        self.expect_kw(Kw::Projecting)?;
        if self.accept_kw(Kw::All) {
            return Ok(Projection {
                ty: ProjectionType::All,
                non_key_attributes: vec![],
            });
        }
        if self.accept_kw(Kw::Keys) {
            self.expect_kw(Kw::Only)?;
            return Ok(Projection {
                ty: ProjectionType::KeysOnly,
                non_key_attributes: vec![],
            });
        }
        self.expect_kw(Kw::Include)?;
        self.expect_char('(')?;
        let mut attrs = vec![self.string()?];
        while self.accept_char(',') {
            attrs.push(self.string()?);
        }
        self.expect_char(')')?;
        Ok(Projection {
            ty: ProjectionType::Include,
            non_key_attributes: attrs,
        })
    }

    /// gsi: string ON key_schema index_projection [billing] [warm]
    fn gsi(&mut self) -> P<Gsi> {
        let name = self.string()?;
        self.expect_kw(Kw::On)?;
        let key_schema = self.key_schema()?;
        let projection = self.index_projection()?;
        let billing = self.optional_billing()?;
        let warm = self.optional_warm()?;
        Ok(Gsi {
            name,
            key_schema,
            projection,
            billing,
            warm,
        })
    }

    fn lsi(&mut self) -> P<Lsi> {
        let name = self.string()?;
        self.expect_kw(Kw::On)?;
        let key_schema = self.key_schema()?;
        let projection = self.index_projection()?;
        Ok(Lsi {
            name,
            key_schema,
            projection,
        })
    }

    fn is_vsi_keyword(&mut self) -> bool {
        self.is_kw(Kw::Vsi) || self.is_kw(Kw::Vector)
    }

    /// vsi_keyword: VSI | VECTOR | VECTOR INDEX
    fn vsi_keyword(&mut self) -> P<()> {
        if self.accept_kw(Kw::Vsi) {
            return Ok(());
        }
        self.expect_kw(Kw::Vector)?;
        self.accept_kw(Kw::Index);
        Ok(())
    }

    fn vector_index(&mut self) -> P<VectorIndexDef> {
        let index_name = self.string()?;
        self.expect_kw(Kw::On)?;
        let vector_attr = self.string()?;
        self.expect_kw(Kw::Dimensions)?;
        let dimensions = parse_ll(&self.whole()?);
        self.expect_kw(Kw::Distance)?;
        let distance = if self.accept_kw(Kw::Cosine) {
            VectorDistance::Cosine
        } else if self.accept_kw(Kw::Euclidean) {
            VectorDistance::Euclidean
        } else if self.accept_kw(Kw::DotProduct) {
            VectorDistance::DotProduct
        } else {
            return self.syntax_error();
        };
        let mut search_schema = Vec::new();
        if self.accept_kw(Kw::Search) {
            self.expect_kw(Kw::Key)?;
            self.expect_char('(')?;
            loop {
                let attr = self.string()?;
                let kind = if self.accept_kw(Kw::Hash) {
                    SearchSchemaKind::Hash
                } else if self.accept_kw(Kw::Filter) {
                    SearchSchemaKind::InlineFilter
                } else {
                    return self.syntax_error();
                };
                search_schema.push(SearchSchemaElem { attr, kind });
                if !self.accept_char(',') {
                    break;
                }
            }
            self.expect_char(')')?;
        }
        let projection = self.index_projection()?;
        Ok(VectorIndexDef {
            index_name,
            vector_attr,
            dimensions,
            distance,
            search_schema,
            projection,
        })
    }

    fn stream_specification(&mut self) -> P<StreamSpec> {
        self.expect_kw(Kw::Stream)?;
        if self.accept_kw(Kw::Disabled) {
            return Ok(StreamSpec {
                enabled: false,
                view_type: StreamViewType::KeysOnly,
            });
        }
        self.expect_char('(')?;
        let view_type = match self.peek() {
            Tok::Kw(Kw::Keys) => {
                self.advance();
                self.expect_kw(Kw::Only)?;
                StreamViewType::KeysOnly
            }
            Tok::Kw(Kw::New) => {
                self.advance();
                self.expect_kw(Kw::Image)?;
                StreamViewType::NewImage
            }
            Tok::Kw(Kw::Old) => {
                self.advance();
                self.expect_kw(Kw::Image)?;
                StreamViewType::OldImage
            }
            Tok::Kw(Kw::Both) => {
                self.advance();
                self.expect_kw(Kw::Images)?;
                StreamViewType::NewAndOldImages
            }
            _ => return self.syntax_error(),
        };
        self.expect_char(')')?;
        Ok(StreamSpec {
            enabled: true,
            view_type,
        })
    }

    fn sse_specification(&mut self) -> P<SseSpec> {
        self.expect_kw(Kw::Sse)?;
        if self.accept_kw(Kw::Disabled) {
            return Ok(SseSpec {
                enabled: false,
                kms_master_key_id: None,
                sse_type: None,
            });
        }
        self.expect_char('(')?;
        self.expect_kw(Kw::Keyid)?;
        let key = self.string()?;
        let sse_type = if self.accept_kw(Kw::Kms) {
            SseType::Kms
        } else if self.accept_kw(Kw::Aes256) {
            SseType::Aes256
        } else {
            return self.syntax_error();
        };
        self.expect_char(')')?;
        Ok(SseSpec {
            enabled: true,
            kms_master_key_id: Some(key),
            sse_type: Some(sse_type),
        })
    }

    /// table_class: TABLE CLASS STANDARD [INFREQUENT ACCESS]
    fn table_class(&mut self) -> P<TableClass> {
        self.expect_kw(Kw::Table)?;
        self.expect_kw(Kw::Class)?;
        self.expect_kw(Kw::Standard)?;
        if self.accept_kw(Kw::Infrequent) {
            self.expect_kw(Kw::Access)?;
            return Ok(TableClass::StandardInfrequentAccess);
        }
        Ok(TableClass::Standard)
    }

    fn optional_table_class(&mut self) -> P<TableClass> {
        if self.is_kw(Kw::Table) {
            self.table_class()
        } else {
            Ok(TableClass::Standard)
        }
    }

    fn deletion_protection(&mut self) -> P<bool> {
        self.expect_kw(Kw::Deletion)?;
        self.expect_kw(Kw::Protection)?;
        if self.accept_kw(Kw::Enabled) {
            Ok(true)
        } else if self.accept_kw(Kw::Disabled) {
            Ok(false)
        } else {
            self.syntax_error()
        }
    }

    fn create_table_command(&mut self) -> P<CommandNode> {
        self.expect_kw(Kw::Create)?;
        self.expect_kw(Kw::Table)?;
        let mut cmd = CreateTableCmd {
            if_not_exists: self.if_not_exists_clause()?,
            nowait: self.accept_kw(Kw::Nowait),
            ..Default::default()
        };
        cmd.table_name = self.string()?;
        self.expect_char('(')?;
        cmd.attribute_definitions = self.attribute_name_type_list()?;
        self.expect_char(')')?;
        self.expect_kw(Kw::Primary)?;
        self.expect_kw(Kw::Key)?;
        cmd.key_schema = self.key_schema()?;
        cmd.billing = self.optional_billing()?;
        cmd.warm = self.optional_warm()?;
        if self.accept_kw(Kw::Gsi) {
            self.expect_char('(')?;
            cmd.gsis.push(self.gsi()?);
            while self.accept_char(',') {
                cmd.gsis.push(self.gsi()?);
            }
            self.expect_char(')')?;
        }
        if self.accept_kw(Kw::Lsi) {
            self.expect_char('(')?;
            cmd.lsis.push(self.lsi()?);
            while self.accept_char(',') {
                cmd.lsis.push(self.lsi()?);
            }
            self.expect_char(')')?;
        }
        if self.is_vsi_keyword() {
            self.vsi_keyword()?;
            self.expect_char('(')?;
            cmd.vector_indexes.push(self.vector_index()?);
            while self.accept_char(',') {
                cmd.vector_indexes.push(self.vector_index()?);
            }
            self.expect_char(')')?;
        }
        if self.is_kw(Kw::Stream) {
            cmd.stream = Some(self.stream_specification()?);
        }
        if self.is_kw(Kw::Sse) {
            cmd.sse = Some(self.sse_specification()?);
        }
        cmd.table_class = self.optional_table_class()?;
        if self.is_kw(Kw::Deletion) {
            cmd.deletion_protection = self.deletion_protection()?;
        }
        if self.accept_kw(Kw::Tags) {
            self.expect_char('(')?;
            loop {
                let key = self.string()?;
                self.expect_char(':')?;
                let value = self.string()?;
                cmd.tags.push(Tag { key, value });
                if !self.accept_char(',') {
                    break;
                }
            }
            self.expect_char(')')?;
        }
        Ok(CommandNode::CreateTable(cmd))
    }

    fn gsi_update_list(&mut self) -> P<Vec<GsiUpdate>> {
        self.expect_char('(')?;
        let mut list = Vec::new();
        loop {
            let index_name = self.string()?;
            self.expect_kw(Kw::Set)?;
            let mut g = GsiUpdate {
                action: GsiAction::Update,
                index_name,
                ..Default::default()
            };
            if self.is_kw(Kw::Billing) {
                g.billing = Some(self.billing_mode_and_throughput()?);
            } else if self.is_kw(Kw::Warm) {
                g.warm = Some(self.warm_throughput()?);
            } else {
                return self.syntax_error();
            }
            list.push(g);
            if !self.accept_char(',') {
                break;
            }
        }
        self.expect_char(')')?;
        Ok(list)
    }

    fn replica_tail(&mut self, table: String, action: ReplicaAction) -> P<CommandNode> {
        let region = self.string()?;
        let table_class = self.optional_table_class()?;
        let throughput_override = if self.is_char('(') {
            Some(self.throughput_override()?)
        } else {
            None
        };
        let mut replica_gsis = Vec::new();
        if self.accept_kw(Kw::Gsi) {
            self.expect_char('(')?;
            loop {
                let index_name = self.string()?;
                let throughput_override = self.throughput_override()?;
                replica_gsis.push(ReplicaGsi {
                    index_name,
                    throughput_override,
                });
                if !self.accept_char(',') {
                    break;
                }
            }
            self.expect_char(')')?;
        }
        Ok(CommandNode::AlterReplica(AlterReplicaCmd {
            table,
            replica: ReplicaUpdate {
                action,
                region,
                table_class: Some(table_class),
                throughput_override,
                replica_gsis,
            },
        }))
    }

    /// '(' WHOLE RCU ')'
    fn throughput_override(&mut self) -> P<ProvisionedThroughputOverride> {
        self.expect_char('(')?;
        let n = parse_ll(&self.whole()?);
        self.expect_kw(Kw::Rcu)?;
        self.expect_char(')')?;
        Ok(ProvisionedThroughputOverride {
            read_capacity_units: n,
        })
    }

    fn alter_vsi_create(
        &mut self,
        table: String,
        attribute_definitions: Vec<AttributeDef>,
    ) -> P<CommandNode> {
        self.vsi_keyword()?;
        let create = self.vector_index()?;
        Ok(CommandNode::AlterTable(AlterTableCmd {
            table,
            attribute_definitions,
            vector_index_updates: vec![VectorIndexUpdateDef {
                create: Some(create),
                drop: None,
            }],
            ..Default::default()
        }))
    }

    fn alter_table_command(&mut self) -> P<CommandNode> {
        self.expect_kw(Kw::Alter)?;
        self.expect_kw(Kw::Table)?;
        let table = self.string()?;
        match self.peek().clone() {
            Tok::Kw(Kw::Set) => {
                self.advance();
                self.alter_set(table)
            }
            Tok::Char('(') => {
                self.advance();
                if self.accept_char(')') {
                    self.expect_kw(Kw::Create)?;
                    return self.alter_vsi_create(table, vec![]);
                }
                let attribute_definitions = self.attribute_name_type_list()?;
                self.expect_char(')')?;
                self.expect_kw(Kw::Create)?;
                if self.accept_kw(Kw::Gsi) {
                    let index_name = self.string()?;
                    self.expect_kw(Kw::On)?;
                    let key_schema = self.key_schema()?;
                    let projection = self.index_projection()?;
                    let billing = self.optional_billing()?;
                    let warm = self.optional_warm()?;
                    let g = GsiUpdate {
                        action: GsiAction::Create,
                        index_name,
                        key_schema: Some(key_schema),
                        projection: Some(projection),
                        billing,
                        warm,
                    };
                    return Ok(CommandNode::AlterTable(AlterTableCmd {
                        table,
                        attribute_definitions,
                        gsi_updates: vec![g],
                        ..Default::default()
                    }));
                }
                if self.is_vsi_keyword() {
                    return self.alter_vsi_create(table, attribute_definitions);
                }
                self.syntax_error()
            }
            Tok::Kw(Kw::Create) => {
                self.advance();
                self.alter_vsi_create(table, vec![])
            }
            Tok::Kw(Kw::Drop) => {
                self.advance();
                if self.accept_kw(Kw::Gsi) {
                    let index_name = self.string()?;
                    let g = GsiUpdate {
                        action: GsiAction::Drop,
                        index_name,
                        ..Default::default()
                    };
                    return Ok(CommandNode::AlterTable(AlterTableCmd {
                        table,
                        gsi_updates: vec![g],
                        ..Default::default()
                    }));
                }
                if self.accept_kw(Kw::Replica) {
                    let region = self.string()?;
                    return Ok(CommandNode::AlterReplica(AlterReplicaCmd {
                        table,
                        replica: ReplicaUpdate {
                            action: ReplicaAction::Drop,
                            region,
                            ..Default::default()
                        },
                    }));
                }
                if self.is_vsi_keyword() {
                    self.vsi_keyword()?;
                    let name = self.string()?;
                    return Ok(CommandNode::AlterTable(AlterTableCmd {
                        table,
                        vector_index_updates: vec![VectorIndexUpdateDef {
                            create: None,
                            drop: Some(name),
                        }],
                        ..Default::default()
                    }));
                }
                self.syntax_error()
            }
            Tok::Kw(Kw::Update) => {
                self.advance();
                if self.accept_kw(Kw::Gsi) {
                    let gsi_updates = self.gsi_update_list()?;
                    return Ok(CommandNode::AlterTable(AlterTableCmd {
                        table,
                        gsi_updates,
                        ..Default::default()
                    }));
                }
                if self.accept_kw(Kw::Replica) {
                    return self.replica_tail(table, ReplicaAction::Update);
                }
                self.syntax_error()
            }
            Tok::Kw(Kw::Add) => {
                self.advance();
                self.expect_kw(Kw::Replica)?;
                self.replica_tail(table, ReplicaAction::Add)
            }
            _ => self.syntax_error(),
        }
    }

    fn alter_set(&mut self, table: String) -> P<CommandNode> {
        let mut cmd = AlterTableCmd {
            table,
            ..Default::default()
        };
        match self.peek().clone() {
            Tok::Kw(Kw::Billing) => {
                let billing = self.billing_mode_and_throughput()?;
                cmd.warm = self.optional_warm()?;
                let provisioned = billing.mode == BillingMode::Provisioned;
                cmd.billing = Some(billing);
                if self.accept_kw(Kw::Update) {
                    self.expect_kw(Kw::Gsi)?;
                    let updates = self.gsi_update_list()?;
                    if provisioned {
                        cmd.gsi_updates = updates;
                    } else {
                        cmd.ignored_gsi_on_demand = true;
                    }
                }
            }
            Tok::Kw(Kw::Warm) => cmd.warm = Some(self.warm_throughput()?),
            Tok::Kw(Kw::Sse) => cmd.sse = Some(self.sse_specification()?),
            Tok::Kw(Kw::Table) => cmd.table_class = Some(self.table_class()?),
            Tok::Kw(Kw::Stream) => cmd.stream = Some(self.stream_specification()?),
            Tok::Kw(Kw::Deletion) => cmd.deletion_protection = Some(self.deletion_protection()?),
            Tok::Kw(Kw::Ttl) => {
                self.advance();
                if self.accept_kw(Kw::Disabled) {
                    return Ok(CommandNode::AlterTtl(AlterTtlCmd {
                        table: cmd.table,
                        enabled: false,
                        attribute: String::new(),
                    }));
                }
                self.expect_char('(')?;
                let attribute = self.string()?;
                self.expect_char(')')?;
                return Ok(CommandNode::AlterTtl(AlterTtlCmd {
                    table: cmd.table,
                    enabled: true,
                    attribute,
                }));
            }
            Tok::Kw(Kw::Pitr) => {
                self.advance();
                let enabled = if self.accept_kw(Kw::Enabled) {
                    true
                } else if self.accept_kw(Kw::Disabled) {
                    false
                } else {
                    return self.syntax_error();
                };
                return Ok(CommandNode::AlterPitr(AlterPitrCmd {
                    table: cmd.table,
                    enabled,
                }));
            }
            _ => return self.syntax_error(),
        }
        Ok(CommandNode::AlterTable(cmd))
    }

    // ---- values -----------------------------------------------------------

    fn param_marker(&mut self) -> Value {
        self.param_count += 1;
        Value::Param(self.param_count)
    }

    /// lee_rhs (allow_empty) / insert_value (!allow_empty).
    fn value(&mut self, allow_empty: bool) -> P<Value> {
        match self.peek().clone() {
            Tok::Whole(n) | Tok::Float(n) => {
                self.advance();
                Ok(Value::N(n))
            }
            Tok::DQuoted(s) => {
                self.advance();
                Ok(Value::S(unquote(&s)))
            }
            Tok::Kw(Kw::True) => {
                self.advance();
                Ok(Value::Bool(true))
            }
            Tok::Kw(Kw::False) => {
                self.advance();
                Ok(Value::Bool(false))
            }
            Tok::Kw(Kw::Null) => {
                self.advance();
                Ok(Value::Null)
            }
            Tok::Char('?') => {
                self.advance();
                Ok(self.param_marker())
            }
            Tok::Char('[') => {
                self.advance();
                if allow_empty && self.accept_char(']') {
                    return Ok(Value::L(vec![]));
                }
                let list = self.lee_rhs_list()?;
                self.expect_char(']')?;
                Ok(Value::L(list))
            }
            Tok::Char('{') => {
                self.advance();
                if allow_empty && self.accept_char('}') {
                    return Ok(Value::M(vec![]));
                }
                let mut map = Vec::new();
                loop {
                    let key = self.string()?;
                    self.expect_char(':')?;
                    let v = self.value(true)?;
                    map.push((key, v));
                    if !self.accept_char(',') {
                        break;
                    }
                }
                self.expect_char('}')?;
                Ok(Value::M(map))
            }
            _ => self.syntax_error(),
        }
    }

    fn lee_rhs(&mut self) -> P<Value> {
        self.value(true)
    }

    fn lee_rhs_list(&mut self) -> P<ValueList> {
        let mut list = vec![self.lee_rhs()?];
        while self.accept_char(',') {
            list.push(self.lee_rhs()?);
        }
        Ok(list)
    }

    /// lee_lhs: string | lee_lhs '[' WHOLE ']' | lee_lhs '.' lee_lhs
    fn lee_lhs(&mut self) -> P<String> {
        let mut path = self.string()?;
        loop {
            if self.accept_char('[') {
                let n = self.whole()?;
                self.expect_char(']')?;
                path = format!("{}[{}]", path, n);
            } else if self.accept_char('.') {
                let rhs = self.string()?;
                path = format!("{}.{}", path, rhs);
            } else {
                return Ok(path);
            }
        }
    }

    fn table_optional_index_name(&mut self) -> P<TableReference> {
        let table = self.string()?;
        let index = if self.accept_char('.') {
            Some(self.string()?)
        } else {
            None
        };
        Ok(TableReference { table, index })
    }

    fn optional_ratelimit(&mut self) -> P<Option<RateLimit>> {
        if !self.accept_kw(Kw::With) {
            return Ok(None);
        }
        self.expect_kw(Kw::Ratelimit)?;
        self.expect_char('(')?;
        let a = parse_double(&self.whole()?);
        let first_rcu = if self.accept_kw(Kw::Rcu) {
            true
        } else if self.accept_kw(Kw::Wcu) {
            false
        } else {
            return self.syntax_error();
        };
        let rl = if self.accept_char(',') {
            let b = parse_double(&self.whole()?);
            self.expect_kw(if first_rcu { Kw::Wcu } else { Kw::Rcu })?;
            if first_rcu {
                RateLimit { rcu: a, wcu: b }
            } else {
                RateLimit { rcu: b, wcu: a }
            }
        } else if first_rcu {
            RateLimit { rcu: a, wcu: 0.0 }
        } else {
            RateLimit { rcu: 0.0, wcu: a }
        };
        self.expect_char(')')?;
        Ok(Some(rl))
    }

    fn optional_return_value(&mut self) -> P<ReturnValue> {
        if !self.accept_kw(Kw::Return) {
            return Ok(ReturnValue::None);
        }
        self.expect_kw(Kw::Values)?;
        let all = if self.accept_kw(Kw::All) {
            true
        } else if self.accept_kw(Kw::Updated) {
            false
        } else {
            return self.syntax_error();
        };
        let old = if self.accept_kw(Kw::Old) {
            true
        } else if self.accept_kw(Kw::New) {
            false
        } else {
            return self.syntax_error();
        };
        Ok(match (all, old) {
            (true, true) => ReturnValue::AllOld,
            (true, false) => ReturnValue::AllNew,
            (false, true) => ReturnValue::UpdatedOld,
            (false, false) => ReturnValue::UpdatedNew,
        })
    }

    // ---- DML --------------------------------------------------------------

    fn write_command(&mut self) -> P<CommandNode> {
        match self.peek().clone() {
            Tok::Kw(Kw::Insert) | Tok::Kw(Kw::Replace) => self.insert_command(),
            Tok::Kw(Kw::Update) | Tok::Kw(Kw::Upsert) => self.update_command(),
            Tok::Kw(Kw::Delete) => self.delete_command(),
            Tok::Kw(Kw::Exists) => {
                self.advance();
                let mut node = self.select_command()?;
                if let CommandNode::Select(s) = &mut node {
                    s.exists_mode = ExistsMode::Exists;
                }
                Ok(node)
            }
            Tok::Kw(Kw::Not) => {
                self.advance();
                self.expect_kw(Kw::Exists)?;
                let mut node = self.select_command()?;
                if let CommandNode::Select(s) = &mut node {
                    s.exists_mode = ExistsMode::NotExists;
                }
                Ok(node)
            }
            _ => self.syntax_error(),
        }
    }

    fn insert_command(&mut self) -> P<CommandNode> {
        let insert = self.accept_kw(Kw::Insert);
        if !insert {
            self.expect_kw(Kw::Replace)?;
        }
        self.expect_kw(Kw::Into)?;
        let table_name = self.string()?;
        self.expect_char('(')?;
        let mut columns = vec![self.string()?];
        while self.accept_char(',') {
            columns.push(self.string()?);
        }
        self.expect_char(')')?;
        self.expect_kw(Kw::Values)?;
        let mut rows = Vec::new();
        loop {
            self.expect_char('(')?;
            let mut row = vec![self.value(false)?];
            while self.accept_char(',') {
                row.push(self.value(false)?);
            }
            self.expect_char(')')?;
            rows.push(row);
            if !self.accept_char(',') {
                break;
            }
        }
        let ratelimit = self.optional_ratelimit()?;
        let return_value = if insert {
            ReturnValue::None
        } else {
            self.optional_return_value()?
        };
        Ok(CommandNode::Insert(InsertCmd {
            table_name,
            columns,
            rows,
            insert,
            ratelimit,
            return_value,
        }))
    }

    fn set_element(&mut self) -> P<UpdateSetElement> {
        let lhs = self.lee_lhs()?;
        if *self.peek() != Tok::Equal {
            return self.syntax_error();
        }
        self.advance();
        let mut e = UpdateSetElement {
            lhs,
            ..Default::default()
        };
        match self.peek().clone() {
            Tok::Unquoted(name) => {
                self.advance();
                e.rhs_attribute = name;
            }
            Tok::Kw(Kw::IfNotExists) => {
                self.advance();
                self.expect_char('(')?;
                e.if_not_exists_lhs = self.lee_lhs()?;
                self.expect_char(',')?;
                e.if_not_exists_value = Some(self.lee_rhs()?);
                self.expect_char(')')?;
            }
            _ => {
                e.rhs_value = Some(self.lee_rhs()?);
                return Ok(e);
            }
        }
        if self.accept_char('+') {
            e.op = "+".into();
            e.rhs_value = Some(self.lee_rhs()?);
        } else if self.accept_char('-') {
            e.op = "-".into();
            e.rhs_value = Some(self.lee_rhs()?);
        }
        Ok(e)
    }

    fn update_command(&mut self) -> P<CommandNode> {
        let upsert = self.accept_kw(Kw::Upsert);
        if !upsert {
            self.expect_kw(Kw::Update)?;
        }
        let tref = self.table_optional_index_name()?;
        let mut cmd = UpdateCmd {
            table: tref.table,
            index: tref.index,
            upsert,
            ..Default::default()
        };
        if self.accept_kw(Kw::Set) {
            cmd.mutation = Mutation::Set;
            cmd.set.push(self.set_element()?);
            while self.accept_char(',') {
                cmd.set.push(self.set_element()?);
            }
        } else if !upsert && self.accept_kw(Kw::Remove) {
            cmd.mutation = Mutation::Remove;
            cmd.remove.push(self.lee_lhs()?);
            while self.accept_char(',') {
                cmd.remove.push(self.lee_lhs()?);
            }
        } else {
            return self.syntax_error();
        }
        cmd.where_ = self.optional_where_clause()?;
        cmd.ratelimit = self.optional_ratelimit()?;
        cmd.return_value = self.optional_return_value()?;
        Ok(CommandNode::Update(cmd))
    }

    fn delete_command(&mut self) -> P<CommandNode> {
        self.expect_kw(Kw::Delete)?;
        self.expect_kw(Kw::From)?;
        let tref = self.table_optional_index_name()?;
        let where_ = self.optional_where_clause()?;
        let ratelimit = self.optional_ratelimit()?;
        let return_value = self.optional_return_value()?;
        Ok(CommandNode::Delete(DeleteCmd {
            table: tref.table,
            index: tref.index,
            where_,
            ratelimit,
            return_value,
        }))
    }

    fn select_command(&mut self) -> P<CommandNode> {
        self.expect_kw(Kw::Select)?;
        let consistent = self.accept_kw(Kw::Consistent);
        let projection = if self.accept_char('*') {
            Vec::new()
        } else {
            let mut list = vec![self.lee_lhs()?];
            while self.accept_char(',') {
                list.push(self.lee_lhs()?);
            }
            list
        };
        self.expect_kw(Kw::From)?;
        let tref = self.table_optional_index_name()?;
        let where_ = self.optional_where_clause()?;
        let (search_vector, topk) = if self.accept_kw(Kw::Nearest) {
            let k = parse_ll(&self.whole()?) as i32;
            self.expect_kw(Kw::To)?;
            let v = self.lee_rhs()?;
            (Some(v), Some(k))
        } else {
            (None, None)
        };
        let rcc = if self.accept_kw(Kw::Return) {
            self.expect_kw(Kw::Consumed)?;
            self.expect_kw(Kw::Capacity)?;
            if self.accept_kw(Kw::Indexes) {
                ReturnConsumedCapacity::Indexes
            } else if self.accept_kw(Kw::Total) {
                ReturnConsumedCapacity::Total
            } else {
                return self.syntax_error();
            }
        } else {
            ReturnConsumedCapacity::None
        };
        let ratelimit = self.optional_ratelimit()?;
        if ratelimit.is_some() && rcc != ReturnConsumedCapacity::None {
            self.semantic_error(
                "You cannot specify both a ratelimit and a return_consumed_capacity.",
            );
            return Ok(CommandNode::Empty);
        }
        Ok(CommandNode::Select(SelectCmd {
            consistent,
            projection,
            table: tref.table,
            index: tref.index,
            where_,
            rcc,
            ratelimit,
            exists_mode: ExistsMode::None,
            search_vector,
            topk,
        }))
    }

    // ---- WHERE ------------------------------------------------------------

    fn optional_where_clause(&mut self) -> P<Option<Where>> {
        if self.accept_kw(Kw::Where) {
            Ok(Some(Where::new(self.logical_or()?)))
        } else {
            Ok(None)
        }
    }

    /// Precedence (low -> high): OR < AND < NOT; all left-associative.
    fn logical_or(&mut self) -> P<LogicalExpr> {
        let mut left = self.logical_and()?;
        while self.accept_kw(Kw::Or) {
            let right = self.logical_and()?;
            left = LogicalExpr::make_or(left, right);
        }
        Ok(left)
    }

    fn logical_and(&mut self) -> P<LogicalExpr> {
        let mut left = self.logical_unary()?;
        while self.accept_kw(Kw::And) {
            let right = self.logical_unary()?;
            left = LogicalExpr::make_and(left, right);
        }
        Ok(left)
    }

    fn logical_unary(&mut self) -> P<LogicalExpr> {
        if self.accept_kw(Kw::Not) {
            let mut e = self.logical_unary()?;
            e.negate();
            return Ok(e);
        }
        if self.accept_char('(') {
            let mut e = self.logical_or()?;
            self.expect_char(')')?;
            e.set_paren_group();
            return Ok(e);
        }
        self.logical_element()
    }

    fn comparison_op(&mut self) -> Option<String> {
        let op = match self.peek() {
            Tok::Equal => "=".to_string(),
            Tok::GreaterThan => ">".to_string(),
            Tok::GreaterThanOrEqual => ">=".to_string(),
            Tok::LessThan => "<".to_string(),
            Tok::LessThanOrEqual => "<=".to_string(),
            Tok::NotEqual(s) => s.clone(),
            _ => return None,
        };
        self.advance();
        Some(op)
    }

    fn paren_lhs(&mut self) -> P<String> {
        self.expect_char('(')?;
        self.lee_lhs()
    }

    fn logical_element(&mut self) -> P<LogicalExpr> {
        match self.peek().clone() {
            Tok::Kw(Kw::AttributeExists) => {
                self.advance();
                let lhs = self.paren_lhs()?;
                self.expect_char(')')?;
                Ok(LogicalExpr::attribute_exists(lhs, true))
            }
            Tok::Kw(Kw::AttributeNotExists) => {
                self.advance();
                let lhs = self.paren_lhs()?;
                self.expect_char(')')?;
                Ok(LogicalExpr::attribute_exists(lhs, false))
            }
            Tok::Kw(Kw::AttributeType) => {
                self.advance();
                let lhs = self.paren_lhs()?;
                self.expect_char(',')?;
                let ty = match self.attribute_type()? {
                    ScalarType::N => "N",
                    ScalarType::S => "S",
                    ScalarType::B => "B",
                };
                self.expect_char(')')?;
                Ok(LogicalExpr::attribute_type(lhs, ty))
            }
            Tok::Kw(Kw::BeginsWith) => {
                self.advance();
                let lhs = self.paren_lhs()?;
                self.expect_char(',')?;
                let prefix = self.string()?;
                self.expect_char(')')?;
                Ok(LogicalExpr::begins_with(lhs, Value::S(prefix)))
            }
            Tok::Kw(Kw::Contains) => {
                self.advance();
                let lhs = self.paren_lhs()?;
                self.expect_char(',')?;
                let operand = self.string()?;
                self.expect_char(')')?;
                Ok(LogicalExpr::contains(lhs, Value::S(operand)))
            }
            Tok::Kw(Kw::Size) => {
                self.advance();
                let lhs = self.paren_lhs()?;
                self.expect_char(')')?;
                let Some(op) = self.comparison_op() else {
                    return self.syntax_error();
                };
                let rhs = self.lee_rhs()?;
                Ok(LogicalExpr::comparison(format!("size({})", lhs), &op, rhs))
            }
            _ => {
                let lhs = self.lee_lhs()?;
                if let Some(op) = self.comparison_op() {
                    let rhs = self.lee_rhs()?;
                    return Ok(LogicalExpr::comparison(lhs, &op, rhs));
                }
                match self.peek().clone() {
                    Tok::Kw(Kw::Is) => {
                        self.advance();
                        if self.accept_kw(Kw::Not) {
                            self.expect_kw(Kw::Null)?;
                            let mut cmp = LogicalExpr::comparison(lhs.clone(), "=", Value::Null);
                            cmp.negate();
                            let exists = LogicalExpr::attribute_exists(lhs, true);
                            Ok(LogicalExpr::make_and(cmp, exists))
                        } else {
                            self.expect_kw(Kw::Null)?;
                            Ok(LogicalExpr::comparison(lhs, "=", Value::Null))
                        }
                    }
                    Tok::Kw(Kw::Between) => {
                        self.advance();
                        let lo = self.lee_rhs()?;
                        self.expect_kw(Kw::And)?;
                        let hi = self.lee_rhs()?;
                        Ok(LogicalExpr::between(lhs, lo, hi))
                    }
                    Tok::Kw(Kw::In) => {
                        self.advance();
                        self.expect_char('(')?;
                        let list = self.lee_rhs_list()?;
                        self.expect_char(')')?;
                        Ok(LogicalExpr::in_list(lhs, list))
                    }
                    _ => self.syntax_error(),
                }
            }
        }
    }

    // ---- transactions -----------------------------------------------------

    fn expect_semicolon(&mut self) -> P<()> {
        if *self.peek() == Tok::Semicolon {
            self.advance();
            Ok(())
        } else {
            self.syntax_error()
        }
    }

    fn transact_command(&mut self) -> P<CommandNode> {
        self.expect_kw(Kw::Begin)?;
        self.expect_semicolon()?;
        let mut cmd = TransactCmd::default();
        match self.peek().clone() {
            Tok::Kw(Kw::Rollback) => {
                self.advance();
                cmd.access = TransactAccess::Read;
                cmd.rollback = true;
                return Ok(CommandNode::Transact(cmd));
            }
            Tok::Kw(Kw::Select) => {
                cmd.access = TransactAccess::Read;
                while self.is_kw(Kw::Select) {
                    cmd.items.push(self.select_command()?);
                    self.expect_semicolon()?;
                }
            }
            t if is_write_start(&t) => {
                cmd.access = TransactAccess::Write;
                loop {
                    let t = self.peek().clone();
                    if !is_write_start(&t) {
                        break;
                    }
                    cmd.items.push(self.write_command()?);
                    self.expect_semicolon()?;
                }
            }
            _ => return self.syntax_error(),
        }
        if self.accept_kw(Kw::Commit) {
            cmd.rollback = false;
        } else if self.accept_kw(Kw::Rollback) {
            cmd.rollback = true;
        } else {
            return self.syntax_error();
        }
        Ok(CommandNode::Transact(cmd))
    }
}

/// The compiler-style caret diagnostic (port of Driver::build_caret).
fn build_caret(input: &str, line: usize, column: usize, lexeme_len: usize) -> String {
    let bytes = input.as_bytes();
    let mut line_start = 0usize;
    let mut cur = 1usize;
    while cur < line && line_start <= bytes.len() {
        match bytes[line_start..].iter().position(|&c| c == b'\n') {
            Some(nl) => {
                line_start += nl + 1;
                cur += 1;
            }
            None => break,
        }
    }
    if line_start > bytes.len() {
        return String::new();
    }
    let rest = &bytes[line_start..];
    let mut src_line = match rest.iter().position(|&c| c == b'\n') {
        Some(nl) => &rest[..nl],
        None => rest,
    };
    if src_line.last() == Some(&b'\r') {
        src_line = &src_line[..src_line.len() - 1];
    }
    if src_line.is_empty() {
        return String::new();
    }
    let mut caret_col = if column > lexeme_len {
        column - lexeme_len
    } else {
        column
    };
    if caret_col < 1 {
        caret_col = 1;
    }
    let limit = (caret_col - 1).min(src_line.len());
    let mut out: Vec<u8> = Vec::with_capacity(src_line.len() + limit + 2);
    out.extend_from_slice(src_line);
    out.push(b'\n');
    for &c in &src_line[..limit] {
        out.push(if c == b'\t' { b'\t' } else { b' ' });
    }
    out.push(b'^');
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expressions::LogicalKind;
    use crate::statement::StatementKind;

    fn ok(s: &str) -> ParsedStatement {
        match parse_one(s) {
            Ok(p) => p,
            Err(e) => panic!("parse failed for {:?}: {}", s, e.message),
        }
    }

    fn err(s: &str) -> Error {
        match parse_one(s) {
            Ok(p) => panic!("expected error for {:?}, got {:?}", s, p),
            Err(e) => e,
        }
    }

    fn select(s: &str) -> SelectCmd {
        match ok(s).command {
            CommandNode::Select(c) => c,
            other => panic!("not a select: {:?}", other),
        }
    }

    fn scan_filter(s: &str) -> String {
        select(s).where_.unwrap().scan_filter_expression(None)
    }

    #[test]
    fn empty_and_comment_only() {
        assert_eq!(ok("").kind(), StatementKind::Empty);
        assert_eq!(ok("-- just a comment").kind(), StatementKind::Empty);
        assert_eq!(ok(";").kind(), StatementKind::Empty);
        let (stmts, e) = parse_batch("");
        assert!(stmts.is_empty() && e.is_none());
        let (stmts, e) = parse_batch(";");
        assert_eq!(stmts.len(), 1);
        assert!(e.is_none());
    }

    #[test]
    fn control_commands() {
        assert_eq!(ok("show version;").kind(), StatementKind::ShowVersion);
        assert_eq!(ok("SHOW CONNECTION").kind(), StatementKind::ShowConnection);
        assert_eq!(ok("quit").kind(), StatementKind::Quit);
        match ok("connect us-west-2 with endpoint \"http://localhost:8000\"").command {
            CommandNode::Connect(c) => {
                assert_eq!(c.region, "us-west-2");
                assert_eq!(c.endpoint, "http://localhost:8000");
            }
            o => panic!("{:?}", o),
        }
        match ok("show tables brief like \"^a.*\"").command {
            CommandNode::ShowTables(c) => {
                assert!(c.brief);
                assert_eq!(c.like, "^a.*");
            }
            o => panic!("{:?}", o),
        }
        match ok("describe \"table with spaces\";").command {
            CommandNode::Describe(d) => assert_eq!(d.table, "table with spaces"),
            o => panic!("{:?}", o),
        }
        match ok("drop table if exists nowait t1").command {
            CommandNode::DropTable(d) => {
                assert_eq!(d.table, "t1");
                assert!(d.if_exists && d.nowait);
            }
            o => panic!("{:?}", o),
        }
    }

    #[test]
    fn show_backups_and_friends() {
        match ok("show backups").command {
            CommandNode::ShowBackups(c) => {
                assert_eq!((c.backup_type.as_str(), c.table.as_str()), ("", ""))
            }
            o => panic!("{:?}", o),
        }
        match ok("show user backups t1").command {
            CommandNode::ShowBackups(c) => {
                assert_eq!((c.backup_type.as_str(), c.table.as_str()), ("user", "t1"))
            }
            o => panic!("{:?}", o),
        }
        assert_eq!(ok("show limits").kind(), StatementKind::ShowLimits);
        match ok("show create table if not exists nowait t").command {
            CommandNode::ShowCreateTable(c) => {
                assert!(c.if_not_exists && c.nowait && c.table == "t")
            }
            o => panic!("{:?}", o),
        }
        match ok("restore table t2 from t1 to pitr \"2024-01-02T03:04:05Z\"").command {
            CommandNode::Restore(r) => {
                assert_eq!(r.kind, RestoreKind::ToPitr);
                assert_eq!((r.target.as_str(), r.source.as_str()), ("t2", "t1"));
                assert_eq!(r.timestamp, "2024-01-02T03:04:05Z");
            }
            o => panic!("{:?}", o),
        }
        match ok("backup table t called b1").command {
            CommandNode::CreateBackup(b) => {
                assert_eq!((b.table.as_str(), b.backup_name.as_str()), ("t", "b1"))
            }
            o => panic!("{:?}", o),
        }
    }

    #[test]
    fn syntax_error_format() {
        let e = err("show tablas;");
        assert_eq!(e.code, Errc::ParseError);
        assert!(
            e.message.starts_with("[1, 13] syntax error at ;"),
            "{}",
            e.message
        );
        let e = err("select * from");
        let lines: Vec<&str> = e.message.split('\n').collect();
        assert_eq!(lines.len(), 3, "{}", e.message);
        assert_eq!(lines[1], "select * from");
        assert_eq!(lines[2], "         ^");
        let e = err("select * frm t");
        assert!(
            e.message
                .starts_with("[1, 13] syntax error at frm\nselect * frm t\n         ^"),
            "{}",
            e.message
        );
    }

    #[test]
    fn batch_location_continuous() {
        let (stmts, e) = parse_batch("show version;\ndescribe backup;");
        assert_eq!(stmts.len(), 1);
        let e = e.unwrap();
        assert!(
            e.message
                .starts_with("[2, 17] syntax error at ;\ndescribe backup;\n               ^"),
            "{}",
            e.message
        );
        let (stmts, e) = parse_batch("show version; drop table nowait mytable;");
        assert!(e.is_none());
        assert_eq!(stmts.len(), 2);
        let (stmts, e) = parse_batch("explain select * from t; show version");
        assert!(e.is_none());
        assert!(stmts[0].explain);
        assert_eq!(stmts[1].kind(), StatementKind::ShowVersion);
    }

    #[test]
    fn explain_not_allowed_on_show() {
        assert!(err("explain show tables")
            .message
            .contains("syntax error at show"));
    }

    #[test]
    fn ratelimit_with_rcc_is_semantic_error() {
        let e =
            err("select * from t return consumed capacity total with ratelimit ( 1 rcu, 1 wcu )");
        assert_eq!(e.code, Errc::InvalidArgument);
        assert_eq!(
            e.message,
            "You cannot specify both a ratelimit and a return_consumed_capacity."
        );
    }

    #[test]
    fn create_table_full() {
        let s = "create table if not exists nowait t ( a number, b string, c binary ) \
                 primary key ( a hash, b range ) billing mode provisioned ( 5 wcu, 10 rcu ) \
                 warm throughput ( 1 rcu, 2 wcu ) \
                 gsi ( g1 on ( (a, c) hash, b range ) projecting include ( x, y ) billing mode on demand throughput unlimited ) \
                 lsi ( l1 on ( a hash, c range ) projecting keys only ) \
                 vector index ( v on emb dimensions 3 distance cosine search key ( a hash, b filter ) projecting all ) \
                 stream ( both images ) sse ( keyid \"k\" kms ) table class standard infrequent access \
                 deletion protection enabled tags ( k1 : v1, \"k 2\" : \"v 2\" )";
        match ok(s).command {
            CommandNode::CreateTable(c) => {
                assert!(c.if_not_exists && c.nowait);
                assert_eq!(c.attribute_definitions.len(), 3);
                assert_eq!(c.key_schema.hash, vec!["a"]);
                assert_eq!(c.key_schema.range, vec!["b"]);
                let b = c.billing.unwrap();
                assert_eq!((b.mode, b.rcu, b.wcu), (BillingMode::Provisioned, 10, 5));
                assert_eq!(c.warm.unwrap().write_units_per_second, 2);
                assert_eq!(c.gsis[0].key_schema.hash, vec!["a", "c"]);
                assert_eq!(
                    c.gsis[0]
                        .billing
                        .as_ref()
                        .unwrap()
                        .od
                        .max_read_request_units,
                    Some(-1)
                );
                assert_eq!(c.lsis[0].projection.ty, ProjectionType::KeysOnly);
                assert_eq!(c.vector_indexes[0].search_schema.len(), 2);
                assert_eq!(c.stream.unwrap().view_type, StreamViewType::NewAndOldImages);
                assert_eq!(c.sse.unwrap().sse_type, Some(SseType::Kms));
                assert_eq!(c.table_class, TableClass::StandardInfrequentAccess);
                assert!(c.deletion_protection);
                assert_eq!(c.tags[1].key, "k 2");
            }
            o => panic!("{:?}", o),
        }
        // range before hash is accepted and bucketed by role.
        match ok("create table t (a number, b number) primary key (b range, a hash)").command {
            CommandNode::CreateTable(c) => {
                assert_eq!(c.key_schema.hash, vec!["a"]);
                assert_eq!(c.key_schema.range, vec!["b"]);
                assert!(c.billing.is_none());
            }
            o => panic!("{:?}", o),
        }
    }

    #[test]
    fn alter_table_forms() {
        match ok(
            "alter table t set billing mode on demand update gsi ( g set billing mode on demand )",
        )
        .command
        {
            CommandNode::AlterTable(a) => {
                assert!(a.ignored_gsi_on_demand);
                assert!(a.gsi_updates.is_empty());
            }
            o => panic!("{:?}", o),
        }
        match ok("alter table t set billing mode provisioned (1 rcu, 2 wcu) update gsi ( g set warm throughput (3 rcu, 4 wcu) )").command {
            CommandNode::AlterTable(a) => assert_eq!(a.gsi_updates.len(), 1),
            o => panic!("{:?}", o),
        }
        match ok("alter table t (x string) create gsi g on (x hash) projecting all").command {
            CommandNode::AlterTable(a) => {
                assert_eq!(a.gsi_updates[0].action, GsiAction::Create);
                assert_eq!(a.attribute_definitions.len(), 1);
            }
            o => panic!("{:?}", o),
        }
        assert!(matches!(
            ok("alter table t drop gsi g").command,
            CommandNode::AlterTable(_)
        ));
        assert!(matches!(ok("alter table t ( ) create vsi v on e dimensions 2 distance euclidean projecting all").command, CommandNode::AlterTable(_)));
        assert!(matches!(
            ok("alter table t drop vector index v").command,
            CommandNode::AlterTable(_)
        ));
        assert!(matches!(
            ok("alter table t set ttl ( exp )").command,
            CommandNode::AlterTtl(AlterTtlCmd { enabled: true, .. })
        ));
        assert!(matches!(
            ok("alter table t set ttl disabled").command,
            CommandNode::AlterTtl(AlterTtlCmd { enabled: false, .. })
        ));
        assert!(matches!(
            ok("alter table t set pitr enabled").command,
            CommandNode::AlterPitr(AlterPitrCmd { enabled: true, .. })
        ));
        match ok("alter table t add replica us-west-2 table class standard ( 5 rcu ) gsi ( g ( 3 rcu ) )").command {
            CommandNode::AlterReplica(r) => {
                assert_eq!(r.replica.action, ReplicaAction::Add);
                assert_eq!(r.replica.region, "us-west-2");
                assert_eq!(r.replica.throughput_override.unwrap().read_capacity_units, 5);
                assert_eq!(r.replica.replica_gsis[0].throughput_override.read_capacity_units, 3);
            }
            o => panic!("{:?}", o),
        }
        assert!(matches!(
            ok("alter table t set deletion protection disabled").command,
            CommandNode::AlterTable(AlterTableCmd {
                deletion_protection: Some(false),
                ..
            })
        ));
    }

    #[test]
    fn insert_and_replace() {
        match ok("insert into t (a, b, c, d) values (1, \"x\", [1, [], {}], {k: true}), (2, null, 1.5, ?)").command {
            CommandNode::Insert(i) => {
                assert!(i.insert);
                assert_eq!(i.rows.len(), 2);
                assert_eq!(i.rows[0][1], Value::S("x".into()));
                assert_eq!(i.rows[0][2], Value::L(vec![Value::N("1".into()), Value::L(vec![]), Value::M(vec![])]));
                assert_eq!(i.rows[1][3], Value::Param(1));
            }
            o => panic!("{:?}", o),
        }
        assert!(err("insert into t (a) values ([])")
            .message
            .contains("syntax error at ]"));
        match ok("replace into t (a) values (1) with ratelimit ( 5 wcu ) return values all old")
            .command
        {
            CommandNode::Insert(i) => {
                assert!(!i.insert);
                assert_eq!(i.return_value, ReturnValue::AllOld);
                assert_eq!(i.ratelimit.unwrap().wcu, 5.0);
            }
            o => panic!("{:?}", o),
        }
        // INSERT has no RETURN VALUES
        assert!(err("insert into t (a) values (1) return values all old")
            .message
            .contains("syntax error at return"));
    }

    #[test]
    fn update_forms() {
        match ok("update t set a = 1, b = c + 2, d = if_not_exists(e, 3) - 1, f = g where k = 1 return values updated new").command {
            CommandNode::Update(u) => {
                assert_eq!(u.set.len(), 4);
                assert_eq!(u.set[1].rhs_attribute, "c");
                assert_eq!(u.set[1].op, "+");
                assert_eq!(u.set[2].if_not_exists_lhs, "e");
                assert_eq!(u.set[2].op, "-");
                assert_eq!(u.set[3].rhs_attribute, "g");
                assert_eq!(u.return_value, ReturnValue::UpdatedNew);
            }
            o => panic!("{:?}", o),
        }
        match ok("update t.idx remove a.b[2], c where x = 1").command {
            CommandNode::Update(u) => {
                assert_eq!(u.index.as_deref(), Some("idx"));
                assert_eq!(u.remove, vec!["a.b[2]", "c"]);
            }
            o => panic!("{:?}", o),
        }
        assert!(err("upsert t remove a")
            .message
            .contains("syntax error at remove"));
        match ok("delete from t where a = 1 with ratelimit (1 wcu, 2 rcu)").command {
            CommandNode::Delete(d) => {
                let rl = d.ratelimit.unwrap();
                assert_eq!((rl.rcu, rl.wcu), (2.0, 1.0));
            }
            o => panic!("{:?}", o),
        }
    }

    #[test]
    fn where_precedence_and_desugaring() {
        assert_eq!(
            scan_filter("select * from t where a = 1 and b = 2 or c = 3"),
            "a = {\"N\":\"1\"} AND b = {\"N\":\"2\"} OR c = {\"N\":\"3\"}"
        );
        assert_eq!(
            scan_filter("select * from t where not a = 1 and b = 2"),
            "NOT (a = {\"N\":\"1\"}) AND b = {\"N\":\"2\"}"
        );
        assert_eq!(
            scan_filter("select * from t where not (a = 1 and b = 2)"),
            "NOT ((a = {\"N\":\"1\"} AND b = {\"N\":\"2\"}))"
        );
        assert_eq!(
            scan_filter("select * from t where a != 1"),
            "NOT (a = {\"N\":\"1\"})"
        );
        assert_eq!(
            scan_filter("select * from t where a is not null"),
            "NOT (a = {\"NULL\":true}) AND attribute_exists(a)"
        );
        assert_eq!(
            scan_filter("select * from t where a between 1 and 2 and attribute_not_exists(b)"),
            "a BETWEEN {\"N\":\"1\"} AND {\"N\":\"2\"} AND NOT (attribute_exists(b))"
        );
        assert_eq!(
            scan_filter("select * from t where size(a.b[0]) > 3"),
            "size(a.b[0]) > {\"N\":\"3\"}"
        );
        assert_eq!(
            scan_filter("select * from t where begins_with(a, \"x\") or contains(b, y)"),
            "begins_with(a, \"{\"S\":\"x\"}\") OR contains(b, \"{\"S\":\"y\"}\")"
        );
        assert_eq!(
            scan_filter("select * from t where a in (1, 2) and attribute_type(b, string)"),
            "a IN ({\"N\":\"1\"}, {\"N\":\"2\"}) AND attribute_type(b, S)"
        );
        let w = select("select * from t where a = 1 and b = 2 and c = 3")
            .where_
            .unwrap();
        match &w.root().kind {
            LogicalKind::And(l) => assert_eq!(l.len(), 3),
            o => panic!("{:?}", o),
        }
    }

    #[test]
    fn select_forms() {
        let s = select("select consistent a, b.c[1] from t.i where x = ? and y = ? return consumed capacity indexes");
        assert!(s.consistent);
        assert_eq!(s.projection, vec!["a", "b.c[1]"]);
        assert_eq!(s.index.as_deref(), Some("i"));
        assert_eq!(s.rcc, ReturnConsumedCapacity::Indexes);
        let s = select("select * from t.v nearest 5 to [0.1, 0.2]");
        assert_eq!(s.topk, Some(5));
        assert!(s.search_vector.is_some());
        let p = ok("select * from t where x = ? and y = ?");
        assert_eq!(crate::parser::count_param_markers(&p), 2);
    }

    #[test]
    fn transactions() {
        match ok("begin; select * from t where a = 1; select * from u where b = 2; commit;").command
        {
            CommandNode::Transact(t) => {
                assert_eq!(t.access, TransactAccess::Read);
                assert_eq!(t.items.len(), 2);
                assert!(!t.rollback);
            }
            o => panic!("{:?}", o),
        }
        match ok("begin; insert into t (a) values (1); not exists select * from t where a = 2; delete from t where a = 3; end").command {
            CommandNode::Transact(t) => {
                assert_eq!(t.access, TransactAccess::Write);
                assert_eq!(t.items.len(), 3);
                match &t.items[1] {
                    CommandNode::Select(s) => assert_eq!(s.exists_mode, ExistsMode::NotExists),
                    o => panic!("{:?}", o),
                }
            }
            o => panic!("{:?}", o),
        }
        assert!(matches!(
            ok("begin; rollback;").command,
            CommandNode::Transact(TransactCmd { rollback: true, .. })
        ));
        assert!(err("begin; commit;")
            .message
            .contains("syntax error at commit"));
        assert!(
            err("begin; select * from t; insert into t (a) values (1); commit;")
                .message
                .contains("syntax error at insert")
        );
        let (stmts, e) = parse_batch("begin; select * from t; commit; show version;");
        assert!(e.is_none());
        assert_eq!(stmts.len(), 2);
    }

    #[test]
    fn help_topics() {
        let topic = |s: &str| match ok(s).command {
            CommandNode::Help(h) => h.topic,
            o => panic!("{:?}", o),
        };
        assert_eq!(topic("help"), HelpTopic::General);
        assert_eq!(topic("help drop table"), HelpTopic::DropTable);
        assert_eq!(topic("help show create table"), HelpTopic::ShowCreateTable);
        assert_eq!(topic("help alter table;"), HelpTopic::AlterTable);
        assert_eq!(topic("HELP DELETE"), HelpTopic::Del);
        assert!(err("help alter").message.contains("syntax error"));
        assert!(err("help foo").message.contains("syntax error at foo"));
    }
}
