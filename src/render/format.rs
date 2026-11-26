// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! Opt-in output formats: JSON, CSV and a text table (port of
//! src/render/format.cpp, ddbsh 0.12.4).
//!
//! A format applies only to rows: the items of a SELECT, a read transaction
//! and RETURN VALUES, and the reports with natural columns (SHOW TABLES, SHOW
//! BACKUPS, SHOW LIMITS). Every other output stays text. `decompose` makes
//! that split; `FormattingSink` writes each fragment as it arrives, holding at
//! most one open result set: a rows part opens one (or continues it), a text
//! part closes it first, and the end of the statement closes it.

use super::{format_score, Renderer};
use crate::client::ItemRow;
use crate::outcome::{
    BulkWriteOutcome, ExecOutcome, OutcomeSink, SelectOutcome, VectorSearchOutcome,
    VectorSearchRow, WriteOutcome,
};
use crate::value::{Value, ValueMap};

/// The output format. `Standard` (the id "default") is ddbsh's own output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputFormat {
    #[default]
    Standard,
    Json,
    Csv,
    Table,
}

/// How JSON encodes a value (and CSV/table cells a map, list or set).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JsonFlavor {
    #[default]
    Plain,
    Dynamodb,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FormatOptions {
    pub format: OutputFormat,
    pub json_flavor: JsonFlavor,
}

impl OutputFormat {
    pub fn id(self) -> &'static str {
        match self {
            OutputFormat::Standard => "default",
            OutputFormat::Json => "json",
            OutputFormat::Csv => "csv",
            OutputFormat::Table => "table",
        }
    }
    /// "default", "json", "csv" or "table", ASCII case-insensitive.
    pub fn parse(id: &str) -> Option<OutputFormat> {
        [
            OutputFormat::Standard,
            OutputFormat::Json,
            OutputFormat::Csv,
            OutputFormat::Table,
        ]
        .into_iter()
        .find(|f| f.id().eq_ignore_ascii_case(id))
    }
}

impl JsonFlavor {
    pub fn id(self) -> &'static str {
        match self {
            JsonFlavor::Plain => "plain",
            JsonFlavor::Dynamodb => "dynamodb",
        }
    }
    /// "plain" or "dynamodb", ASCII case-insensitive.
    pub fn parse(id: &str) -> Option<JsonFlavor> {
        [JsonFlavor::Plain, JsonFlavor::Dynamodb]
            .into_iter()
            .find(|f| f.id().eq_ignore_ascii_case(id))
    }
}

// ---- Decomposition -----------------------------------------------------------

/// Output that is text in every format (the Renderer's stdout / stderr bytes).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TextPart {
    pub out: String,
    pub err: String,
}

/// Rows a format lays out: items, or report rows.
#[derive(Debug, Clone, PartialEq)]
pub struct RowsPart {
    pub rows: Vec<ItemRow>,
    /// Columns known before the rows are seen (a report's complete list, or a
    /// SELECT's hint). May be empty.
    pub columns: Vec<String>,
    /// Reports and vector search: `columns` is complete and ordered, and a
    /// JSON element keeps that order instead of byte order.
    pub fixed_columns: bool,
    /// An API page ended with this part (the table format prints a box here).
    pub group_end: bool,
}

impl Default for RowsPart {
    fn default() -> Self {
        RowsPart {
            rows: vec![],
            columns: vec![],
            fixed_columns: false,
            group_end: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum OutputPart {
    Text(TextPart),
    Rows(RowsPart),
}

fn push_text(parts: &mut Vec<OutputPart>, outcome: &ExecOutcome) {
    let text = TextPart {
        out: Renderer.render(outcome),
        err: Renderer.render_stderr(outcome),
    };
    if !text.out.is_empty() || !text.err.is_empty() {
        parts.push(OutputPart::Text(text));
    }
}

fn find_attribute<'a>(row: &'a ValueMap, name: &str) -> Option<&'a Value> {
    row.iter().find(|(k, _)| k == name).map(|(_, v)| v)
}

/// `score`, or `score` with as many leading underscores as it takes not to
/// collide with an item attribute.
fn score_column(rows: &[VectorSearchRow]) -> String {
    let mut name = "score".to_string();
    while rows
        .iter()
        .any(|r| find_attribute(&r.item.attributes, &name).is_some())
    {
        name.insert(0, '_');
    }
    name
}

fn decompose_into(parts: &mut Vec<OutputPart>, outcome: &ExecOutcome) {
    match outcome {
        ExecOutcome::Select(s) => {
            parts.push(OutputPart::Rows(RowsPart {
                // The Renderer skips an empty item; so do the formats.
                rows: s
                    .items
                    .iter()
                    .filter(|i| !i.attributes.is_empty())
                    .cloned()
                    .collect(),
                columns: s.column_hint.clone(),
                ..Default::default()
            }));
            if s.consumed_capacity.is_some() {
                push_text(
                    parts,
                    &ExecOutcome::Select(SelectOutcome {
                        consumed_capacity: s.consumed_capacity.clone(),
                        ..Default::default()
                    }),
                );
            }
        }
        ExecOutcome::VectorSearch(v) => {
            let score = score_column(&v.rows);
            let mut rows = RowsPart {
                fixed_columns: true,
                ..Default::default()
            };
            for r in &v.rows {
                let mut item = r.item.clone();
                item.attributes
                    .push((score.clone(), Value::N(format_score(r.score))));
                rows.rows.push(item);
            }
            let mut tracker = ColumnTracker::default();
            tracker.add(&rows);
            rows.columns = tracker
                .columns()
                .iter()
                .filter(|c| **c != score)
                .cloned()
                .collect();
            rows.columns.push(score);
            parts.push(OutputPart::Rows(rows));
            if v.vector_capacity.is_some() {
                push_text(
                    parts,
                    &ExecOutcome::VectorSearch(VectorSearchOutcome {
                        rows: vec![],
                        vector_capacity: v.vector_capacity.clone(),
                    }),
                );
            }
        }
        ExecOutcome::TransactRead(t) => parts.push(OutputPart::Rows(RowsPart {
            rows: t.items.clone(),
            ..Default::default()
        })),
        ExecOutcome::Write(w) => {
            for (i, token) in w.tokens.iter().enumerate() {
                push_text(
                    parts,
                    &ExecOutcome::Write(WriteOutcome {
                        tokens: vec![token.clone()],
                        items: vec![],
                    }),
                );
                if let Some(Some(item)) = w.items.get(i) {
                    parts.push(OutputPart::Rows(RowsPart {
                        rows: vec![item.clone()],
                        ..Default::default()
                    }));
                }
            }
        }
        ExecOutcome::BulkWrite(b) => {
            if !b.returned_items.is_empty() {
                parts.push(OutputPart::Rows(RowsPart {
                    rows: b.returned_items.clone(),
                    // A partial fragment is one item, not a page.
                    group_end: !b.partial,
                    ..Default::default()
                }));
            }
            if !b.partial {
                push_text(
                    parts,
                    &ExecOutcome::BulkWrite(BulkWriteOutcome {
                        returned_items: vec![],
                        ..b.clone()
                    }),
                );
            }
        }
        ExecOutcome::Report(r) => match &r.table {
            Some(table) => parts.push(OutputPart::Rows(RowsPart {
                rows: table
                    .rows
                    .iter()
                    .map(|attributes| ItemRow {
                        attributes: attributes.clone(),
                    })
                    .collect(),
                columns: table.columns.clone(),
                fixed_columns: true,
                group_end: table.group_end,
            })),
            None => push_text(parts, outcome),
        },
        ExecOutcome::Multi(list) => {
            for part in list {
                decompose_into(parts, part);
            }
        }
        _ => push_text(parts, outcome),
    }
}

/// Split an outcome into its text and its rows, in output order.
pub fn decompose(outcome: &ExecOutcome) -> Vec<OutputPart> {
    let mut parts = Vec::new();
    decompose_into(&mut parts, outcome);
    parts
}

/// The column list of one result set. Columns are only ever appended.
#[derive(Debug, Default, Clone)]
pub struct ColumnTracker {
    columns: Vec<String>,
}

impl ColumnTracker {
    fn has(&self, name: &str) -> bool {
        self.columns.iter().any(|c| c == name)
    }
    /// Add a part's columns, then every attribute of its rows not yet present,
    /// in byte order. Returns the index of the first column this call added.
    pub fn add(&mut self, part: &RowsPart) -> usize {
        let first_new = self.columns.len();
        for c in &part.columns {
            if !self.has(c) {
                self.columns.push(c.clone());
            }
        }
        let mut fresh: Vec<&String> = Vec::new();
        for row in &part.rows {
            for (name, _) in &row.attributes {
                if !self.has(name) && !fresh.contains(&name) {
                    fresh.push(name);
                }
            }
        }
        fresh.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
        self.columns.extend(fresh.into_iter().cloned());
        first_new
    }
    pub fn columns(&self) -> &[String] {
        &self.columns
    }
    pub fn clear(&mut self) {
        self.columns.clear();
    }
}

// ---- Value encoding ------------------------------------------------------------

/// Append `text` as a JSON string (cJSON-compatible escaping).
pub fn append_json_string(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// True when `text` matches the RFC 8259 number grammar.
pub fn is_json_number(text: &str) -> bool {
    let b = text.as_bytes();
    let n = b.len();
    let digit = |at: usize| at < n && b[at].is_ascii_digit();
    let mut i = 0;
    if i < n && b[i] == b'-' {
        i += 1;
    }
    if !digit(i) {
        return false;
    }
    if b[i] == b'0' {
        i += 1;
    } else {
        while digit(i) {
            i += 1;
        }
    }
    if i < n && b[i] == b'.' {
        i += 1;
        if !digit(i) {
            return false;
        }
        while digit(i) {
            i += 1;
        }
    }
    if i < n && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < n && (b[i] == b'+' || b[i] == b'-') {
            i += 1;
        }
        if !digit(i) {
            return false;
        }
        while digit(i) {
            i += 1;
        }
    }
    i == n
}

fn append_json_number(out: &mut String, digits: &str) {
    if is_json_number(digits) {
        out.push_str(digits);
    } else {
        append_json_string(out, digits);
    }
}

fn append_string_array(out: &mut String, items: &[String], numbers: bool) {
    out.push('[');
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        if numbers {
            append_json_number(out, item);
        } else {
            append_json_string(out, item);
        }
    }
    out.push(']');
}

fn byte_ordered(map: &ValueMap) -> Vec<&(String, Value)> {
    let mut entries: Vec<&(String, Value)> = map.iter().collect();
    entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    entries
}

fn append_map(out: &mut String, map: &ValueMap, flavor: JsonFlavor) {
    out.push('{');
    for (i, (k, v)) in byte_ordered(map).into_iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        append_json_string(out, k);
        out.push_str(": ");
        append_json_value(out, v, flavor);
    }
    out.push('}');
}

fn append_tagged_string(out: &mut String, tag: &str, text: &str) {
    out.push_str(&format!("{{\"{}\": ", tag));
    append_json_string(out, text);
    out.push('}');
}

fn append_plain(out: &mut String, v: &Value) {
    match v {
        Value::S(s) | Value::B(s) => append_json_string(out, s),
        Value::N(n) => append_json_number(out, n),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Null | Value::Param(_) => out.push_str("null"),
        Value::L(list) => {
            out.push('[');
            for (i, e) in list.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                append_plain(out, e);
            }
            out.push(']');
        }
        Value::M(map) => append_map(out, map, JsonFlavor::Plain),
        Value::SS(set) | Value::BS(set) => append_string_array(out, set, false),
        Value::NS(set) => append_string_array(out, set, true),
    }
}

fn append_typed(out: &mut String, v: &Value) {
    match v {
        Value::S(s) => append_tagged_string(out, "S", s),
        Value::N(n) => append_tagged_string(out, "N", n),
        Value::B(b) => append_tagged_string(out, "B", b),
        Value::Bool(b) => out.push_str(if *b {
            "{\"BOOL\": true}"
        } else {
            "{\"BOOL\": false}"
        }),
        Value::Null | Value::Param(_) => out.push_str("{\"NULL\": true}"),
        Value::L(list) => {
            out.push_str("{\"L\": [");
            for (i, e) in list.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                append_typed(out, e);
            }
            out.push_str("]}");
        }
        Value::M(map) => {
            out.push_str("{\"M\": ");
            append_map(out, map, JsonFlavor::Dynamodb);
            out.push('}');
        }
        // The wire encoding carries NS members as strings.
        Value::SS(set) | Value::NS(set) | Value::BS(set) => {
            let tag = match v {
                Value::SS(_) => "SS",
                Value::NS(_) => "NS",
                _ => "BS",
            };
            out.push_str(&format!("{{\"{}\": ", tag));
            append_string_array(out, set, false);
            out.push('}');
        }
    }
}

/// Append one value as JSON in the given flavor.
pub fn append_json_value(out: &mut String, value: &Value, flavor: JsonFlavor) {
    match flavor {
        JsonFlavor::Plain => append_plain(out, value),
        JsonFlavor::Dynamodb => append_typed(out, value),
    }
}

/// The text of one CSV or table cell.
pub fn format_cell(value: &Value, flavor: JsonFlavor) -> String {
    match value {
        Value::S(s) | Value::N(s) | Value::B(s) => s.clone(),
        Value::Bool(b) => if *b { "true" } else { "false" }.into(),
        Value::Null | Value::Param(_) => "null".into(),
        _ => {
            let mut out = String::new();
            append_json_value(&mut out, value, flavor);
            out
        }
    }
}

fn append_json_row(out: &mut String, part: &RowsPart, row: &ItemRow, flavor: JsonFlavor) {
    let mut first = true;
    let mut entry = |out: &mut String, name: &str, value: &Value| {
        if !first {
            out.push_str(", ");
        }
        first = false;
        append_json_string(out, name);
        out.push_str(": ");
        append_json_value(out, value, flavor);
    };
    out.push('{');
    if part.fixed_columns {
        for column in &part.columns {
            if let Some(v) = find_attribute(&row.attributes, column) {
                entry(out, column, v);
            }
        }
    } else {
        for (k, v) in byte_ordered(&row.attributes) {
            entry(out, k, v);
        }
    }
    out.push('}');
}

/// One CSV field: quoted iff it holds `,`, `"`, CR or LF; `"` doubled.
fn append_csv_field(out: &mut String, text: &str) {
    if !text.contains([',', '"', '\r', '\n']) {
        out.push_str(text);
        return;
    }
    out.push('"');
    out.push_str(&text.replace('"', "\"\""));
    out.push('"');
}

/// A table cell's display text: control characters as their JSON escapes.
fn escape_controls(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                out.push_str(&format!("\\u{:04x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out
}

/// Display width in code points (wide/combining characters not accounted for).
fn display_width(text: &str) -> usize {
    text.chars().count()
}

struct TableCell {
    text: String,
    right: bool,
}

/// One box: a rule, the header, a rule, then (with `rows`) the rows and a rule.
fn draw_box(columns: &[String], rows: Option<&[ValueMap]>, flavor: JsonFlavor) -> String {
    let header: Vec<String> = columns.iter().map(|c| escape_controls(c)).collect();
    let mut widths: Vec<usize> = header.iter().map(|h| display_width(h)).collect();
    let mut cells: Vec<Vec<TableCell>> = Vec::new();
    for row in rows.unwrap_or(&[]) {
        let mut line = Vec::with_capacity(columns.len());
        for (i, column) in columns.iter().enumerate() {
            let cell = match find_attribute(row, column) {
                Some(v) => TableCell {
                    text: escape_controls(&format_cell(v, flavor)),
                    right: matches!(v, Value::N(_)),
                },
                None => TableCell {
                    text: String::new(),
                    right: false,
                },
            };
            widths[i] = widths[i].max(display_width(&cell.text));
            line.push(cell);
        }
        cells.push(line);
    }

    let mut rule = String::from("+");
    for w in &widths {
        rule.push_str(&"-".repeat(w + 2));
        rule.push('+');
    }
    rule.push('\n');

    let draw_line = |line: &[TableCell]| -> String {
        let mut out = String::from("|");
        for (i, cell) in line.iter().enumerate() {
            let pad = " ".repeat(widths[i] - display_width(&cell.text));
            out.push(' ');
            if cell.right {
                out.push_str(&pad);
                out.push_str(&cell.text);
            } else {
                out.push_str(&cell.text);
                out.push_str(&pad);
            }
            out.push_str(" |");
        }
        out.push('\n');
        out
    };

    let header_line: Vec<TableCell> = header
        .into_iter()
        .map(|text| TableCell { text, right: false })
        .collect();
    let mut out = rule.clone() + &draw_line(&header_line) + &rule;
    if rows.is_some() {
        for line in &cells {
            out += &draw_line(line);
        }
        out += &rule;
    }
    out
}

// ---- Streaming -------------------------------------------------------------

/// Which stream a piece of formatted output belongs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Out,
    Err,
}

/// Receives each piece of output, in order: (stream, text, current options).
/// Never called with empty text.
pub type Writer<'a> = Box<dyn FnMut(Stream, &str, &FormatOptions) + 'a>;

/// An OutcomeSink that formats each fragment as it arrives and hands the bytes
/// to a writer. In `Standard` it writes exactly the Renderer's bytes per
/// fragment; in the other formats it follows the open-result-set rule.
pub struct FormattingSink<'a> {
    options: FormatOptions,
    writer: Writer<'a>,
    open: bool,
    tracker: ColumnTracker,
    json_elements: usize,
    csv_header_size: usize,
    pending: Vec<ValueMap>,
    box_printed: bool,
}

impl<'a> FormattingSink<'a> {
    pub fn new(options: FormatOptions, writer: Writer<'a>) -> Self {
        FormattingSink {
            options,
            writer,
            open: false,
            tracker: ColumnTracker::default(),
            json_elements: 0,
            csv_header_size: 0,
            pending: vec![],
            box_printed: false,
        }
    }

    pub fn options(&self) -> FormatOptions {
        self.options
    }

    /// Change the selection for the statements that follow. An open result set
    /// is closed first, in the old format.
    pub fn set_options(&mut self, options: FormatOptions) {
        self.close_set();
        self.options = options;
    }

    fn write(&mut self, stream: Stream, text: &str) {
        if !text.is_empty() {
            (self.writer)(stream, text, &self.options);
        }
    }

    fn close_set(&mut self) {
        if !self.open {
            return;
        }
        self.open = false;
        match self.options.format {
            OutputFormat::Json => {
                let closing = if self.json_elements == 0 {
                    "[]\n"
                } else {
                    "\n]\n"
                };
                self.write(Stream::Out, closing);
            }
            OutputFormat::Table => {
                self.flush_table();
                if !self.box_printed && !self.tracker.columns().is_empty() {
                    let b = draw_box(self.tracker.columns(), None, self.options.json_flavor);
                    self.write(Stream::Out, &b);
                }
            }
            OutputFormat::Csv | OutputFormat::Standard => {}
        }
        self.tracker.clear();
        self.pending.clear();
    }

    fn start_set(&mut self) {
        self.tracker.clear();
        self.json_elements = 0;
        self.csv_header_size = 0;
        self.pending.clear();
        self.box_printed = false;
    }

    fn flush_table(&mut self) {
        if self.pending.is_empty() {
            return;
        }
        if !self.tracker.columns().is_empty() {
            let b = draw_box(
                self.tracker.columns(),
                Some(&self.pending),
                self.options.json_flavor,
            );
            self.write(Stream::Out, &b);
            self.box_printed = true;
        }
        self.pending.clear();
    }

    fn rows(&mut self, part: &RowsPart) {
        let flavor = self.options.json_flavor;
        match self.options.format {
            OutputFormat::Json => {
                for row in &part.rows {
                    let mut chunk = String::from(if self.json_elements == 0 {
                        "[\n  "
                    } else {
                        ",\n  "
                    });
                    append_json_row(&mut chunk, part, row, flavor);
                    self.write(Stream::Out, &chunk);
                    self.json_elements += 1;
                }
            }
            OutputFormat::Csv => {
                self.tracker.add(part);
                let columns = self.tracker.columns().to_vec();
                let mut chunk = String::new();
                if columns.len() > self.csv_header_size {
                    for (i, c) in columns.iter().enumerate() {
                        if i > 0 {
                            chunk.push(',');
                        }
                        append_csv_field(&mut chunk, c);
                    }
                    chunk.push('\n');
                    self.csv_header_size = columns.len();
                }
                for row in &part.rows {
                    for (i, c) in columns.iter().enumerate() {
                        if i > 0 {
                            chunk.push(',');
                        }
                        if let Some(v) = find_attribute(&row.attributes, c) {
                            append_csv_field(&mut chunk, &format_cell(v, flavor));
                        }
                    }
                    chunk.push('\n');
                }
                self.write(Stream::Out, &chunk);
            }
            OutputFormat::Table => {
                self.tracker.add(part);
                self.pending
                    .extend(part.rows.iter().map(|r| r.attributes.clone()));
                if part.group_end {
                    self.flush_table();
                }
            }
            OutputFormat::Standard => {}
        }
    }
}

impl OutcomeSink for FormattingSink<'_> {
    fn emit(&mut self, fragment: ExecOutcome) {
        if self.options.format == OutputFormat::Standard {
            let out = Renderer.render(&fragment);
            let err = Renderer.render_stderr(&fragment);
            self.write(Stream::Out, &out);
            self.write(Stream::Err, &err);
            return;
        }
        for part in decompose(&fragment) {
            match part {
                OutputPart::Text(text) => {
                    self.close_set();
                    self.write(Stream::Out, &text.out);
                    self.write(Stream::Err, &text.err);
                }
                OutputPart::Rows(rows) => {
                    if !self.open {
                        self.open = true;
                        self.start_set();
                    }
                    self.rows(&rows);
                }
            }
        }
    }

    fn end_statement(&mut self) {
        self.close_set();
    }
}

/// Format one buffered outcome as a whole statement: the stdout bytes.
pub fn render_formatted(outcome: &ExecOutcome, options: FormatOptions) -> String {
    let mut out = String::new();
    {
        let mut sink = FormattingSink::new(
            options,
            Box::new(|stream, text, _| {
                if stream == Stream::Out {
                    out.push_str(text);
                }
            }),
        );
        sink.emit(outcome.clone());
        sink.end_statement();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outcome::{ReportOutcome, ReportTable, TransactReadOutcome};

    fn row(attrs: Vec<(&str, Value)>) -> ItemRow {
        ItemRow {
            attributes: attrs.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        }
    }

    fn n(s: &str) -> Value {
        Value::N(s.into())
    }
    fn s(s: &str) -> Value {
        Value::S(s.into())
    }

    fn opts(format: OutputFormat, json_flavor: JsonFlavor) -> FormatOptions {
        FormatOptions {
            format,
            json_flavor,
        }
    }

    fn sample() -> ExecOutcome {
        ExecOutcome::Select(SelectOutcome {
            items: vec![
                row(vec![
                    ("pk", n("1")),
                    ("rk", n("1")),
                    ("a", s("hello")),
                    ("b", n("42")),
                    ("c", Value::Bool(true)),
                    ("d", Value::Null),
                ]),
                row(vec![
                    ("pk", n("1")),
                    ("rk", n("2")),
                    ("a", s("say \"\"hi\"\", ok")),
                    ("f", s("C:\\dir")),
                ]),
                row(vec![
                    ("pk", n("1")),
                    ("rk", n("4")),
                    ("ns", Value::NS(vec!["1".into(), "3".into(), "20".into()])),
                    ("ss", Value::SS(vec!["a".into(), "b".into()])),
                    ("l", Value::L(vec![n("1"), s("two"), Value::M(vec![])])),
                ]),
            ],
            consumed_capacity: None,
            column_hint: vec!["pk".into(), "rk".into()],
        })
    }

    #[test]
    fn ids() {
        assert_eq!(OutputFormat::parse("JSON"), Some(OutputFormat::Json));
        assert_eq!(OutputFormat::parse("default"), Some(OutputFormat::Standard));
        assert_eq!(OutputFormat::parse("xml"), None);
        assert_eq!(JsonFlavor::parse("DynamoDB"), Some(JsonFlavor::Dynamodb));
        assert_eq!(JsonFlavor::parse("typed"), None);
    }

    #[test]
    fn json_plain_and_typed() {
        assert_eq!(
            render_formatted(&sample(), opts(OutputFormat::Json, JsonFlavor::Plain)),
            "[\n  {\"a\": \"hello\", \"b\": 42, \"c\": true, \"d\": null, \"pk\": 1, \"rk\": 1},\n  \
             {\"a\": \"say \\\"\\\"hi\\\"\\\", ok\", \"f\": \"C:\\\\dir\", \"pk\": 1, \"rk\": 2},\n  \
             {\"l\": [1, \"two\", {}], \"ns\": [1, 3, 20], \"pk\": 1, \"rk\": 4, \"ss\": [\"a\", \"b\"]}\n]\n"
        );
        let typed = render_formatted(&sample(), opts(OutputFormat::Json, JsonFlavor::Dynamodb));
        assert!(typed.contains(
            "{\"l\": {\"L\": [{\"N\": \"1\"}, {\"S\": \"two\"}, {\"M\": {}}]}, \"ns\": {\"NS\": [\"1\", \"3\", \"20\"]}"
        ));
        assert!(typed.contains("\"c\": {\"BOOL\": true}, \"d\": {\"NULL\": true}"));
    }

    #[test]
    fn csv_and_table() {
        assert_eq!(
            render_formatted(&sample(), opts(OutputFormat::Csv, JsonFlavor::Plain)),
            "pk,rk,a,b,c,d,f,l,ns,ss\n\
             1,1,hello,42,true,null,,,,\n\
             1,2,\"say \"\"\"\"hi\"\"\"\", ok\",,,,C:\\dir,,,\n\
             1,4,,,,,,\"[1, \"\"two\"\", {}]\",\"[1, 3, 20]\",\"[\"\"a\"\", \"\"b\"\"]\"\n"
        );
        let one = ExecOutcome::Select(SelectOutcome {
            items: vec![row(vec![("pk", n("1")), ("a", s("hello"))])],
            consumed_capacity: None,
            column_hint: vec!["pk".into()],
        });
        assert_eq!(
            render_formatted(&one, opts(OutputFormat::Table, JsonFlavor::Plain)),
            "+----+-------+\n| pk | a     |\n+----+-------+\n|  1 | hello |\n+----+-------+\n"
        );
    }

    #[test]
    fn empty_results() {
        let empty = ExecOutcome::Select(SelectOutcome {
            column_hint: vec!["pk".into(), "rk".into()],
            ..Default::default()
        });
        assert_eq!(
            render_formatted(&empty, opts(OutputFormat::Json, JsonFlavor::Plain)),
            "[]\n"
        );
        assert_eq!(
            render_formatted(&empty, opts(OutputFormat::Csv, JsonFlavor::Plain)),
            "pk,rk\n"
        );
        assert_eq!(
            render_formatted(&empty, opts(OutputFormat::Table, JsonFlavor::Plain)),
            "+----+----+\n| pk | rk |\n+----+----+\n"
        );
    }

    #[test]
    fn text_closes_a_result_set() {
        let multi = ExecOutcome::Multi(vec![
            ExecOutcome::Select(SelectOutcome {
                items: vec![row(vec![("a", s("x"))])],
                ..Default::default()
            }),
            ExecOutcome::status("INSERT"),
            ExecOutcome::TransactRead(TransactReadOutcome {
                items: vec![row(vec![("a", s("y"))]), ItemRow::default()],
            }),
        ]);
        assert_eq!(
            render_formatted(&multi, opts(OutputFormat::Json, JsonFlavor::Plain)),
            "[\n  {\"a\": \"x\"}\n]\nINSERT\n[\n  {\"a\": \"y\"},\n  {}\n]\n"
        );
    }

    #[test]
    fn report_table_keeps_column_order() {
        let report = ExecOutcome::Report(ReportOutcome {
            lines: vec!["t | ACTIVE".into()],
            table: Some(ReportTable {
                columns: vec!["TableName".into(), "TableStatus".into(), "GSICount".into()],
                rows: vec![vec![
                    ("TableName".into(), s("t")),
                    ("TableStatus".into(), s("ACTIVE")),
                    ("GSICount".into(), n("0")),
                ]],
                group_end: true,
            }),
        });
        assert_eq!(
            render_formatted(&report, opts(OutputFormat::Json, JsonFlavor::Plain)),
            "[\n  {\"TableName\": \"t\", \"TableStatus\": \"ACTIVE\", \"GSICount\": 0}\n]\n"
        );
        assert_eq!(
            render_formatted(&report, opts(OutputFormat::Standard, JsonFlavor::Plain)),
            "t | ACTIVE\n"
        );
    }

    #[test]
    fn bulk_write_partial_fragments() {
        let mut out = String::new();
        {
            let mut sink = FormattingSink::new(
                opts(OutputFormat::Json, JsonFlavor::Plain),
                Box::new(|_, t, _| out.push_str(t)),
            );
            for v in ["1", "2"] {
                sink.emit(ExecOutcome::BulkWrite(BulkWriteOutcome {
                    returned_items: vec![row(vec![("a", n(v))])],
                    partial: true,
                    ..Default::default()
                }));
            }
            sink.emit(ExecOutcome::BulkWrite(BulkWriteOutcome {
                read: 2,
                modified: 2,
                ..Default::default()
            }));
            sink.end_statement();
        }
        assert_eq!(
            out,
            "[\n  {\"a\": 1},\n  {\"a\": 2}\n]\nUPDATE (2 read, 2 modified, 0 ccf)\n"
        );
    }

    #[test]
    fn vector_search_score_column() {
        let v = ExecOutcome::VectorSearch(VectorSearchOutcome {
            rows: vec![VectorSearchRow {
                item: row(vec![("score", n("7")), ("id", s("a"))]),
                score: 0.5,
            }],
            vector_capacity: None,
        });
        assert_eq!(
            render_formatted(&v, opts(OutputFormat::Csv, JsonFlavor::Plain)),
            "id,score,_score\na,7,0.5\n"
        );
    }

    #[test]
    fn controls_escaped_in_table_cells() {
        let o = ExecOutcome::Select(SelectOutcome {
            items: vec![row(vec![("a", s("x\ny\u{7f}"))])],
            ..Default::default()
        });
        assert_eq!(
            render_formatted(&o, opts(OutputFormat::Table, JsonFlavor::Plain)),
            "+------------+\n| a          |\n+------------+\n| x\\ny\\u007f |\n+------------+\n"
        );
        assert!(!is_json_number("01"));
        assert!(is_json_number("-1.5e+3"));
        assert!(!is_json_number("1."));
    }
}
