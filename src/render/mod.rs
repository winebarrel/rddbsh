//! Output + EXPLAIN formatting (port of src/render/renderer.cpp).
use crate::client::{ConsumedCapacity, ItemRow};
use crate::error::Error;
use crate::outcome::*;
use crate::value::Value;

fn quote_if_required(input: &str) -> String {
    if input
        .chars()
        .any(|c| matches!(c, ' ' | '\t' | ',' | ':' | '\n' | '[' | ']' | '{' | '}'))
    {
        format!("\"{}\"", input)
    } else {
        input.to_string()
    }
}

fn sorted(entries: &[(String, Value)]) -> Vec<&(String, Value)> {
    let mut v: Vec<&(String, Value)> = entries.iter().collect();
    v.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    v
}

/// `{a: 1, b: x}` with attributes alpha-sorted; empty item renders as "".
pub fn show_item(item: &ItemRow) -> String {
    if item.attributes.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = sorted(&item.attributes)
        .into_iter()
        .map(|(k, v)| format!("{}: {}", quote_if_required(k), render_value(v)))
        .collect();
    format!("{{{}}}", parts.join(", "))
}

/// Port of the original's trim_double regex
/// `^([\d,]+)$|^([\d,]+)\.0*$|^([\d,]+\.[0-9]*?)0*$` -> `$1$2$3`.
pub fn trim_double(input: &str) -> String {
    let is_int = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || c == ',');
    if is_int(input) {
        return input.to_string();
    }
    if let Some((int_part, frac)) = input.split_once('.') {
        if is_int(int_part) && frac.chars().all(|c| c.is_ascii_digit()) {
            if frac.chars().all(|c| c == '0') {
                return int_part.to_string();
            }
            return format!("{}.{}", int_part, frac.trim_end_matches('0'));
        }
    }
    input.to_string()
}

/// std::to_string(double) == printf("%f").
fn to_string_f64(v: f64) -> String {
    format!("{:.6}", v)
}

fn render_consumed_capacity(cc: &ConsumedCapacity) -> String {
    let any = cc.capacity_units.is_some()
        || cc.read_capacity_units.is_some()
        || cc.write_capacity_units.is_some();
    let Some(table) = &cc.table_name else {
        return String::new();
    };
    if !any {
        return String::new();
    }
    let field = |v: Option<f64>| {
        v.map(|x| trim_double(&to_string_f64(x)))
            .unwrap_or_else(|| "0".into())
    };
    format!(
        "SELECT ({}, {}, {}, {})\n",
        table,
        field(cc.capacity_units),
        field(cc.read_capacity_units),
        field(cc.write_capacity_units)
    )
}

fn format_score(score: f64) -> String {
    let s = to_string_f64(score);
    match s.strip_prefix('-') {
        Some(rest) => format!("-{}", trim_double(rest)),
        None => trim_double(&s),
    }
}

fn render_select(s: &SelectOutcome) -> String {
    let mut out = String::new();
    for item in &s.items {
        let line = show_item(item);
        if !line.is_empty() {
            out += &line;
            out.push('\n');
        }
    }
    if let Some(cc) = &s.consumed_capacity {
        out += &render_consumed_capacity(cc);
    }
    out
}

fn render_vector_search(v: &VectorSearchOutcome) -> String {
    let mut out = String::new();
    for row in &v.rows {
        let item = show_item(&row.item);
        if item.is_empty() {
            out += &format!("score {}\n", format_score(row.score));
        } else {
            out += &format!("{} score {}\n", item, format_score(row.score));
        }
    }
    if let Some(vc) = &v.vector_capacity {
        let field = |x: Option<f64>| {
            x.map(|x| trim_double(&to_string_f64(x)))
                .unwrap_or_else(|| "0".into())
        };
        out += &format!(
            "SELECT ({} vector search request bytes, {} vector write request bytes)\n",
            field(vc.vector_search_request_bytes),
            field(vc.vector_write_request_bytes)
        );
    }
    out
}

fn render_write(w: &WriteOutcome) -> String {
    let mut out = String::new();
    for (i, token) in w.tokens.iter().enumerate() {
        match w.items.get(i) {
            Some(Some(item)) => out += &format!("{}: {}\n", token, show_item(item)),
            _ => out += &format!("{}\n", token),
        }
    }
    out
}

fn render_bulk_write(b: &BulkWriteOutcome) -> String {
    let mut out = String::new();
    for item in &b.returned_items {
        out += &show_item(item);
        out.push('\n');
    }
    let verb = match b.verb {
        BulkVerb::Update => "UPDATE",
        BulkVerb::Upsert => "UPSERT",
        BulkVerb::Del => "DELETE",
    };
    out += &format!(
        "{} ({} read, {} modified, {} ccf)\n",
        verb, b.read, b.modified, b.ccf
    );
    out
}

/// Pretty EXPLAIN: every tab becomes three spaces.
pub fn explain_string(s: &str) -> String {
    s.replace('\t', "   ")
}

/// Compact EXPLAIN: `[\n\r][\t]*` -> " ", then `:\t` -> ": ".
pub fn strip_newlines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\n' || c == '\r' {
            while chars.peek() == Some(&'\t') {
                chars.next();
            }
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out.replace(":\t", ": ")
}

fn render_explain(e: &ExplainOutcome) -> String {
    let mut out = String::new();
    for block in &e.blocks {
        let body = match block.format {
            ExplainFormat::Compact => strip_newlines(&block.body),
            ExplainFormat::Pretty => explain_string(&block.body),
        };
        out += &format!("{}({})\n", block.api_name, body);
    }
    if let Some(t) = &e.trailing_status {
        out += t;
        out.push('\n');
    }
    out
}

fn render_outcome(outcome: &ExecOutcome) -> String {
    match outcome {
        ExecOutcome::Status(s) => format!("{}\n", s.token),
        ExecOutcome::Report(r) => r.lines.iter().map(|l| format!("{}\n", l)).collect(),
        ExecOutcome::Empty => String::new(),
        ExecOutcome::Select(s) => render_select(s),
        ExecOutcome::Write(w) => render_write(w),
        ExecOutcome::Explain(e) => render_explain(e),
        ExecOutcome::BulkWrite(b) => render_bulk_write(b),
        ExecOutcome::Transact(t) => format!("{}\n", t.token),
        ExecOutcome::TransactRead(t) => t
            .items
            .iter()
            .map(|i| format!("{}\n", show_item(i)))
            .collect(),
        ExecOutcome::VectorSearch(v) => render_vector_search(v),
        ExecOutcome::Multi(parts) => parts.iter().map(render_outcome).collect(),
    }
}

fn render_outcome_stderr(outcome: &ExecOutcome) -> String {
    match outcome {
        ExecOutcome::Status(s) => s.diagnostics.iter().map(|l| format!("{}\n", l)).collect(),
        ExecOutcome::Transact(t) => t.diagnostics.iter().map(|l| format!("{}\n", l)).collect(),
        ExecOutcome::Multi(parts) => parts.iter().map(render_outcome_stderr).collect(),
        _ => String::new(),
    }
}

/// Render one attribute value (nested maps use `k:v` without a space).
pub fn render_value(v: &Value) -> String {
    match v {
        Value::S(s) => quote_if_required(s),
        Value::N(n) => n.clone(),
        Value::Bool(b) => if *b { "TRUE" } else { "FALSE" }.into(),
        Value::Null | Value::Param(_) => "NULL".into(),
        Value::L(list) => {
            let parts: Vec<String> = list.iter().map(render_value).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::M(map) => {
            let parts: Vec<String> = sorted(map)
                .into_iter()
                .map(|(k, v)| format!("{}:{}", quote_if_required(k), render_value(v)))
                .collect();
            format!("{{{}}}", parts.join(", "))
        }
        Value::B(_) | Value::SS(_) | Value::NS(_) | Value::BS(_) => "Unknown Type".into(),
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Renderer;

impl Renderer {
    /// stdout bytes for an outcome.
    pub fn render(&self, outcome: &ExecOutcome) -> String {
        render_outcome(outcome)
    }
    /// stderr bytes for an outcome (diagnostics).
    pub fn render_stderr(&self, outcome: &ExecOutcome) -> String {
        render_outcome_stderr(outcome)
    }
    pub fn render_error(&self, error: &Error) -> String {
        format!("{}\n", error.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(attrs: Vec<(&str, Value)>) -> ItemRow {
        ItemRow {
            attributes: attrs.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        }
    }

    #[test]
    fn item_rendering_sorted_and_quoted() {
        let item = row(vec![
            ("b", Value::S("hello world".into())),
            ("a", Value::N("3".into())),
            ("c", Value::Bool(true)),
            ("d", Value::Null),
            (
                "e",
                Value::M(vec![
                    ("y".into(), Value::N("2".into())),
                    ("x".into(), Value::S("v".into())),
                ]),
            ),
            (
                "f",
                Value::L(vec![Value::N("1".into()), Value::Bool(false)]),
            ),
            ("g", Value::L(vec![])),
            ("h", Value::M(vec![])),
        ]);
        assert_eq!(
            show_item(&item),
            "{a: 3, b: \"hello world\", c: TRUE, d: NULL, e: {x:v, y:2}, f: [1, FALSE], g: [], h: {}}"
        );
    }

    #[test]
    fn select_with_capacity() {
        let o = ExecOutcome::Select(SelectOutcome {
            items: vec![row(vec![("a", Value::N("1".into()))]), ItemRow::default()],
            consumed_capacity: Some(ConsumedCapacity {
                table_name: Some("t".into()),
                capacity_units: Some(0.5),
                read_capacity_units: None,
                write_capacity_units: Some(2.0),
            }),
        });
        assert_eq!(Renderer.render(&o), "{a: 1}\nSELECT (t, 0.5, 0, 2)\n");
    }

    #[test]
    fn trim_double_cases() {
        assert_eq!(trim_double("1.000000"), "1");
        assert_eq!(trim_double("0.500000"), "0.5");
        assert_eq!(trim_double("12"), "12");
        assert_eq!(trim_double("-1.500000"), "-1.500000");
        assert_eq!(format_score(-1.5), "-1.5");
    }

    #[test]
    fn explain_formats() {
        let o = ExecOutcome::Explain(ExplainOutcome {
            blocks: vec![
                ExplainBlock {
                    api_name: "GetItem".into(),
                    body: "{\n\t\"a\":\t1\n}".into(),
                    format: ExplainFormat::Pretty,
                },
                ExplainBlock {
                    api_name: "DescribeTable".into(),
                    body: "{\n\t\"TableName\":\t\"t\"\n}".into(),
                    format: ExplainFormat::Compact,
                },
            ],
            trailing_status: Some("CREATE".into()),
        });
        assert_eq!(
            Renderer.render(&o),
            "GetItem({\n   \"a\":   1\n})\nDescribeTable({ \"TableName\": \"t\" })\nCREATE\n"
        );
    }

    #[test]
    fn bulk_and_write() {
        let b = ExecOutcome::BulkWrite(BulkWriteOutcome {
            verb: BulkVerb::Del,
            read: 3,
            modified: 2,
            ccf: 1,
            ..Default::default()
        });
        assert_eq!(Renderer.render(&b), "DELETE (3 read, 2 modified, 1 ccf)\n");
        let w = ExecOutcome::Write(WriteOutcome {
            tokens: vec!["INSERT".into(), "INSERT".into()],
            items: vec![Some(row(vec![("a", Value::N("1".into()))])), None],
        });
        assert_eq!(Renderer.render(&w), "INSERT: {a: 1}\nINSERT\n");
    }

    #[test]
    fn stderr_diagnostics() {
        let s = ExecOutcome::Status(StatusOutcome {
            token: "ALTER".into(),
            diagnostics: vec!["warn".into()],
        });
        assert_eq!(Renderer.render(&s), "ALTER\n");
        assert_eq!(Renderer.render_stderr(&s), "warn\n");
    }
}
