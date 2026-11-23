// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! Tokenizer (port of src/parser/lexer.l).
//!
//! Reproduces flex's longest-match / earliest-rule semantics over the original
//! rule set:
//!   - `--` and `//` single-line comments; whitespace discarded.
//!   - T_TIMESTAMP (`"YYYY-MM-DDThh:mm:ssZ"`) before the general quoted string.
//!   - T_DQUOTED_STRING with SQL-style `""` doubling (verbatim lexeme kept).
//!   - T_WHOLE_NUMBER `-?[1-9][0-9]*` or `0`.
//!   - T_FLOATING_POINT_NUMBER with UPPERCASE `E` exponents only.
//!   - identifiers `[a-zA-Z0-9_][a-zA-Z0-9_-]*` with case-insensitive keyword
//!     lookup (miss => T_UNQUOTED_STRING).
//!   - operators `<= >= <> != < > = ;`, and a catch-all single character.
//!
//! Every token carries the driver cursor state *after* it was scanned (line,
//! column, last matched lexeme — which for end-of-input is the last
//! whitespace/comment/token text), which is what the syntax-error message uses.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kw {
    Access,
    Add,
    Aes256,
    All,
    Alter,
    And,
    As,
    AttributeExists,
    AttributeNotExists,
    AttributeType,
    AttributeTypeBinary,
    AttributeTypeNumber,
    AttributeTypeString,
    Backup,
    Backups,
    Begin,
    BeginsWith,
    Between,
    Billing,
    Both,
    Brief,
    Called,
    Capacity,
    Check,
    Class,
    Commit,
    Connect,
    Connection,
    Consistent,
    Consumed,
    Contains,
    Cosine,
    Create,
    Delete,
    Deletion,
    Demand,
    Describe,
    Dimensions,
    Disabled,
    Distance,
    DotProduct,
    Drop,
    Enabled,
    Endpoint,
    Euclidean,
    Exists,
    Explain,
    False,
    Filter,
    From,
    Global,
    Gsi,
    Hash,
    Help,
    If,
    IfNotExists,
    Image,
    Images,
    In,
    Include,
    Index,
    Indexes,
    Infrequent,
    Insert,
    Into,
    Is,
    Key,
    Keyid,
    Keys,
    Kms,
    Like,
    Limit,
    Limits,
    Lsi,
    Mode,
    Nearest,
    New,
    Not,
    Nowait,
    Null,
    Number,
    Old,
    On,
    Only,
    Or,
    Pitr,
    Primary,
    Projecting,
    Protection,
    Provisioned,
    Quit,
    Range,
    Ratelimit,
    Rcu,
    Remove,
    Replace,
    Replica,
    Restore,
    Return,
    Rollback,
    Select,
    Search,
    Set,
    Show,
    Size,
    Sse,
    Standard,
    Stream,
    Table,
    Tables,
    Tags,
    Throughput,
    To,
    Total,
    True,
    Ttl,
    Unlimited,
    Update,
    Updated,
    Upsert,
    Values,
    Vector,
    Version,
    Vsi,
    Warm,
    Wcu,
    Where,
    With,
}

/// Case-insensitive keyword lookup (port of the lexer.l keyword table,
/// including its aliases: end=>COMMIT, numeric=>NUMBER, number/string/binary
/// => attribute-type tokens).
pub fn keyword(text: &str) -> Option<Kw> {
    use Kw::*;
    let lower = text.to_ascii_lowercase();
    Some(match lower.as_str() {
        "access" => Access,
        "add" => Add,
        "aes256" => Aes256,
        "all" => All,
        "alter" => Alter,
        "and" => And,
        "as" => As,
        "attribute_exists" => AttributeExists,
        "attribute_not_exists" => AttributeNotExists,
        "attribute_type" => AttributeType,
        "backup" => Backup,
        "backups" => Backups,
        "begin" => Begin,
        "begins_with" => BeginsWith,
        "between" => Between,
        "billing" => Billing,
        "binary" => AttributeTypeBinary,
        "both" => Both,
        "brief" => Brief,
        "called" => Called,
        "capacity" => Capacity,
        "check" => Check,
        "class" => Class,
        "commit" => Commit,
        "connect" => Connect,
        "connection" => Connection,
        "consistent" => Consistent,
        "consumed" => Consumed,
        "contains" => Contains,
        "cosine" => Cosine,
        "create" => Create,
        "delete" => Delete,
        "deletion" => Deletion,
        "demand" => Demand,
        "describe" => Describe,
        "dimensions" => Dimensions,
        "disabled" => Disabled,
        "distance" => Distance,
        "dot_product" => DotProduct,
        "drop" => Drop,
        "enabled" => Enabled,
        "end" => Commit,
        "endpoint" => Endpoint,
        "euclidean" => Euclidean,
        "exists" => Exists,
        "explain" => Explain,
        "false" => False,
        "filter" => Filter,
        "from" => From,
        "global" => Global,
        "gsi" => Gsi,
        "hash" => Hash,
        "help" => Help,
        "if" => If,
        "if_not_exists" => IfNotExists,
        "image" => Image,
        "images" => Images,
        "in" => In,
        "include" => Include,
        "index" => Index,
        "indexes" => Indexes,
        "infrequent" => Infrequent,
        "insert" => Insert,
        "into" => Into,
        "is" => Is,
        "key" => Key,
        "keyid" => Keyid,
        "keys" => Keys,
        "kms" => Kms,
        "like" => Like,
        "limit" => Limit,
        "limits" => Limits,
        "lsi" => Lsi,
        "mode" => Mode,
        "nearest" => Nearest,
        "new" => New,
        "not" => Not,
        "nowait" => Nowait,
        "null" => Null,
        "number" => AttributeTypeNumber,
        "numeric" => Number,
        "old" => Old,
        "on" => On,
        "only" => Only,
        "or" => Or,
        "pitr" => Pitr,
        "primary" => Primary,
        "projecting" => Projecting,
        "protection" => Protection,
        "provisioned" => Provisioned,
        "quit" => Quit,
        "range" => Range,
        "ratelimit" => Ratelimit,
        "rcu" => Rcu,
        "remove" => Remove,
        "replace" => Replace,
        "replica" => Replica,
        "restore" => Restore,
        "return" => Return,
        "rollback" => Rollback,
        "select" => Select,
        "search" => Search,
        "set" => Set,
        "show" => Show,
        "size" => Size,
        "sse" => Sse,
        "standard" => Standard,
        "stream" => Stream,
        "string" => AttributeTypeString,
        "table" => Table,
        "tables" => Tables,
        "tags" => Tags,
        "throughput" => Throughput,
        "to" => To,
        "total" => Total,
        "true" => True,
        "ttl" => Ttl,
        "unlimited" => Unlimited,
        "update" => Update,
        "updated" => Updated,
        "upsert" => Upsert,
        "values" => Values,
        "vector" => Vector,
        "version" => Version,
        "vsi" => Vsi,
        "warm" => Warm,
        "wcu" => Wcu,
        "where" => Where,
        "with" => With,
        _ => return None,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    /// end of input
    End,
    Semicolon,
    Kw(Kw),
    /// verbatim lexeme, quotes included
    DQuoted(String),
    /// verbatim lexeme, quotes included
    Timestamp(String),
    Whole(String),
    Float(String),
    Unquoted(String),
    Equal,
    GreaterThan,
    GreaterThanOrEqual,
    LessThan,
    LessThanOrEqual,
    /// `!=` or `<>` (verbatim)
    NotEqual(String),
    /// any other single character: ( ) , . [ ] { } : + - * ? ...
    Char(char),
}

/// A token plus the driver cursor state after it was scanned.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub tok: Tok,
    /// 1-based line of the cursor after this token
    pub line: usize,
    /// 1-based column of the cursor after this token
    pub column: usize,
    /// verbatim text of the last matched lexeme (for End: the last rule matched
    /// before end of input, possibly whitespace or a comment; empty if none)
    pub last_lexeme: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Rule {
    Skip,
    Timestamp,
    DQuoted,
    Whole,
    Float,
    Ident,
    Le,
    Ge,
    Ne,
    Lt,
    Gt,
    Eq,
    Semi,
    AnyChar,
}

fn count_digits(b: &[u8], from: usize) -> usize {
    b[from.min(b.len())..]
        .iter()
        .take_while(|c| c.is_ascii_digit())
        .count()
}

fn line_comment_len(b: &[u8]) -> usize {
    b.iter().take_while(|&&c| c != b'\n').count()
}

fn timestamp_len(b: &[u8]) -> usize {
    // "YYYY-MM-DDThh:mm:ssZ"
    const PAT: &[u8] = b"\"dddd-dd-ddTdd:dd:ddZ\"";
    if b.len() < PAT.len() {
        return 0;
    }
    for (i, &p) in PAT.iter().enumerate() {
        let c = b[i];
        let ok = if p == b'd' {
            c.is_ascii_digit()
        } else {
            c == p
        };
        if !ok {
            return 0;
        }
    }
    PAT.len()
}

fn dquoted_len(b: &[u8]) -> usize {
    if b.first() != Some(&b'"') {
        return 0;
    }
    let mut best = 0;
    let mut pos = 1;
    while pos < b.len() {
        if b[pos] == b'"' {
            best = pos + 1; // this quote can close the string
            if pos + 1 < b.len() && b[pos + 1] == b'"' {
                pos += 2; // or be the first half of a doubled quote
                continue;
            }
            break;
        }
        pos += 1;
    }
    best
}

fn whole_len(b: &[u8]) -> usize {
    let sign = usize::from(b.first() == Some(&b'-'));
    match b.get(sign) {
        Some(c) if (b'1'..=b'9').contains(c) => sign + 1 + count_digits(b, sign + 1),
        _ => 0,
    }
}

fn exponent_len(b: &[u8], at: usize) -> usize {
    if b.get(at) != Some(&b'E') {
        return 0;
    }
    let mut p = at + 1;
    if matches!(b.get(p), Some(b'-') | Some(b'+')) {
        p += 1;
    }
    let d = count_digits(b, p);
    if d == 0 {
        0
    } else {
        p + d - at
    }
}

fn float_len(b: &[u8]) -> usize {
    let sign = usize::from(b.first() == Some(&b'-'));
    let s = &b[sign..];
    let mut best = 0;
    let d1 = count_digits(s, 0);
    if d1 > 0 {
        if s.get(d1) == Some(&b'.') {
            let d2 = count_digits(s, d1 + 1);
            let base = d1 + 1 + d2;
            best = best.max(base + exponent_len(s, base));
        }
        let e = exponent_len(s, d1);
        if e > 0 {
            best = best.max(d1 + e);
        }
    } else if s.first() == Some(&b'.') {
        let d = count_digits(s, 1);
        if d > 0 {
            let base = 1 + d;
            best = best.max(base);
            best = best.max(base + exponent_len(s, base));
        }
    }
    if best == 0 {
        0
    } else {
        sign + best
    }
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

fn ident_len(b: &[u8]) -> usize {
    match b.first() {
        Some(&c) if is_ident_start(c) => {
            1 + b[1..]
                .iter()
                .take_while(|&&c| is_ident_start(c) || c == b'-')
                .count()
        }
        _ => 0,
    }
}

/// Scan the whole input into tokens (always ending with an `End` token).
pub fn tokenize(input: &str) -> Vec<Token> {
    let bytes = input.as_bytes();
    let mut pos = 0;
    let mut line = 1usize;
    let mut column = 1usize;
    let mut last_lexeme = String::new();
    let mut tokens = Vec::new();

    while pos < bytes.len() {
        let b = &bytes[pos..];
        // Candidates in rule order; the longest wins, ties go to the earliest.
        let mut best_len = 0usize;
        let mut best_rule = Rule::AnyChar;
        let mut consider = |len: usize, rule: Rule| {
            if len > best_len {
                best_len = len;
                best_rule = rule;
            }
        };
        if b.starts_with(b"--") || b.starts_with(b"//") {
            consider(line_comment_len(b), Rule::Skip);
        }
        consider(
            b.iter()
                .take_while(|c| matches!(c, b' ' | b'\t' | b'\r' | b'\n'))
                .count(),
            Rule::Skip,
        );
        consider(timestamp_len(b), Rule::Timestamp);
        consider(dquoted_len(b), Rule::DQuoted);
        consider(whole_len(b), Rule::Whole);
        consider(usize::from(b[0] == b'0'), Rule::Whole);
        consider(float_len(b), Rule::Float);
        consider(ident_len(b), Rule::Ident);
        if b.starts_with(b"<=") {
            consider(2, Rule::Le);
        }
        if b.starts_with(b">=") {
            consider(2, Rule::Ge);
        }
        if b.starts_with(b"<>") || b.starts_with(b"!=") {
            consider(2, Rule::Ne);
        }
        match b[0] {
            b'<' => consider(1, Rule::Lt),
            b'>' => consider(1, Rule::Gt),
            b'=' => consider(1, Rule::Eq),
            b';' => consider(1, Rule::Semi),
            _ => {}
        }
        if best_len == 0 {
            // catch-all: one (UTF-8) character
            let ch_len = input[pos..].chars().next().map(char::len_utf8).unwrap_or(1);
            best_len = ch_len;
            best_rule = Rule::AnyChar;
        }

        let text = &input[pos..pos + best_len];
        // YY_USER_ACTION: record the lexeme, advance the cursor per byte.
        last_lexeme = text.to_string();
        for &c in text.as_bytes() {
            if c == b'\n' || c == b'\r' {
                line += 1;
                column = 1;
            } else {
                column += 1;
            }
        }
        pos += best_len;

        let tok = match best_rule {
            Rule::Skip => continue,
            Rule::Timestamp => Tok::Timestamp(text.to_string()),
            Rule::DQuoted => Tok::DQuoted(text.to_string()),
            Rule::Whole => Tok::Whole(text.to_string()),
            Rule::Float => Tok::Float(text.to_string()),
            Rule::Ident => match keyword(text) {
                Some(kw) => Tok::Kw(kw),
                None => Tok::Unquoted(text.to_string()),
            },
            Rule::Le => Tok::LessThanOrEqual,
            Rule::Ge => Tok::GreaterThanOrEqual,
            Rule::Ne => Tok::NotEqual(text.to_string()),
            Rule::Lt => Tok::LessThan,
            Rule::Gt => Tok::GreaterThan,
            Rule::Eq => Tok::Equal,
            Rule::Semi => Tok::Semicolon,
            Rule::AnyChar => Tok::Char(text.chars().next().unwrap_or('\0')),
        };
        tokens.push(Token {
            tok,
            line,
            column,
            last_lexeme: last_lexeme.clone(),
        });
    }
    tokens.push(Token {
        tok: Tok::End,
        line,
        column,
        last_lexeme,
    });
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Tok> {
        tokenize(s).into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn keywords_case_insensitive() {
        assert_eq!(toks("SeLeCt"), vec![Tok::Kw(Kw::Select), Tok::End]);
        assert_eq!(toks("end"), vec![Tok::Kw(Kw::Commit), Tok::End]);
        assert_eq!(
            toks("number"),
            vec![Tok::Kw(Kw::AttributeTypeNumber), Tok::End]
        );
    }

    #[test]
    fn identifiers_with_hyphens_and_digits() {
        assert_eq!(
            toks("us-east-1"),
            vec![Tok::Unquoted("us-east-1".into()), Tok::End]
        );
        assert_eq!(toks("01"), vec![Tok::Unquoted("01".into()), Tok::End]);
        assert_eq!(toks("1-2"), vec![Tok::Unquoted("1-2".into()), Tok::End]);
        assert_eq!(toks("1e5"), vec![Tok::Unquoted("1e5".into()), Tok::End]);
    }

    #[test]
    fn numbers() {
        assert_eq!(toks("3"), vec![Tok::Whole("3".into()), Tok::End]);
        assert_eq!(toks("-3"), vec![Tok::Whole("-3".into()), Tok::End]);
        assert_eq!(toks("0"), vec![Tok::Whole("0".into()), Tok::End]);
        assert_eq!(toks("1.5"), vec![Tok::Float("1.5".into()), Tok::End]);
        assert_eq!(toks("-.5"), vec![Tok::Float("-.5".into()), Tok::End]);
        assert_eq!(toks("1E5"), vec![Tok::Float("1E5".into()), Tok::End]);
        assert_eq!(toks("1.5E-3"), vec![Tok::Float("1.5E-3".into()), Tok::End]);
        assert_eq!(toks("3."), vec![Tok::Float("3.".into()), Tok::End]);
        assert_eq!(
            toks("- 1"),
            vec![Tok::Char('-'), Tok::Whole("1".into()), Tok::End]
        );
    }

    #[test]
    fn strings_and_timestamps() {
        assert_eq!(
            toks("\"a\"\"b\""),
            vec![Tok::DQuoted("\"a\"\"b\"".into()), Tok::End]
        );
        assert_eq!(
            toks("\"2024-01-02T03:04:05Z\""),
            vec![Tok::Timestamp("\"2024-01-02T03:04:05Z\"".into()), Tok::End]
        );
        assert_eq!(
            toks("\"a\"\""),
            vec![Tok::DQuoted("\"a\"".into()), Tok::Char('"'), Tok::End]
        );
    }

    #[test]
    fn operators_and_comments() {
        assert_eq!(
            toks("a <= b -- trailing\n!= <> // x"),
            vec![
                Tok::Unquoted("a".into()),
                Tok::LessThanOrEqual,
                Tok::Unquoted("b".into()),
                Tok::NotEqual("!=".into()),
                Tok::NotEqual("<>".into()),
                Tok::End
            ]
        );
    }

    #[test]
    fn cursor_tracking() {
        let t = tokenize("show tablas;");
        assert_eq!(t[1].last_lexeme, "tablas");
        assert_eq!((t[1].line, t[1].column), (1, 12));
        let end = t.last().unwrap();
        assert_eq!(end.last_lexeme, ";");
        let t = tokenize("a\n  ");
        assert_eq!(t.last().unwrap().last_lexeme, "\n  ");
        assert_eq!((t.last().unwrap().line, t.last().unwrap().column), (2, 3));
    }
}
