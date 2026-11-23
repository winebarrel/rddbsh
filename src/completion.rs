// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0

//! Tab completion for the interactive shell: keywords everywhere, and table
//! names where the grammar expects a table (after FROM, INTO, UPDATE, ...).
//! Terminal-free: the CLI wires this into its line editor.

use crate::parser::lexer::KEYWORDS;

/// Words after which a table name is expected.
const TABLE_CONTEXT: &[&str] = &[
    "from", "into", "update", "upsert", "describe", "desc", "table", "exists", "nowait",
];

/// Characters that end a word for completion purposes.
fn is_delimiter(c: char) -> bool {
    c.is_whitespace() || matches!(c, '(' | ')' | ',' | ';' | '=' | '<' | '>')
}

/// Byte offset where the word under the cursor starts.
pub fn word_start(line: &str, pos: usize) -> usize {
    line[..pos]
        .char_indices()
        .rev()
        .find(|&(_, c)| is_delimiter(c))
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0)
}

/// The word before the one under the cursor (lowercased), if any.
fn previous_word(line: &str, start: usize) -> Option<String> {
    let before = line[..start].trim_end_matches(|c: char| c.is_whitespace());
    if before.is_empty() || before.ends_with(|c: char| is_delimiter(c)) {
        return None;
    }
    let s = word_start(before, before.len());
    Some(before[s..].to_ascii_lowercase())
}

/// Does the cursor's word sit where a table name is expected?
pub fn wants_table(line: &str, pos: usize) -> bool {
    let start = word_start(line, pos);
    previous_word(line, start).is_some_and(|w| TABLE_CONTEXT.contains(&w.as_str()))
}

/// Completion candidates for the word under the cursor: `(start, candidates)`.
/// `tables` is called only when a table name is expected; matching table
/// names then take precedence, and keywords are offered only if none match
/// (e.g. `create table if ...`).
pub fn complete(
    line: &str,
    pos: usize,
    tables: impl FnOnce() -> Vec<String>,
) -> (usize, Vec<String>) {
    let start = word_start(line, pos);
    let prefix = &line[start..pos];
    let mut out: Vec<String> = Vec::new();
    if wants_table(line, pos) {
        out.extend(tables().into_iter().filter(|t| t.starts_with(prefix)));
        if !out.is_empty() {
            return (start, out);
        }
    }
    if !prefix.is_empty() {
        let lower = prefix.to_ascii_lowercase();
        let upper = prefix
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase());
        let mut kws: Vec<String> = KEYWORDS
            .iter()
            .filter(|k| k.starts_with(&lower))
            .map(|k| {
                if upper {
                    k.to_ascii_uppercase()
                } else {
                    k.to_string()
                }
            })
            .collect();
        kws.sort();
        out.extend(kws);
    }
    out.dedup();
    (start, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables() -> Vec<String> {
        vec!["movies".into(), "music".into(), "orders".into()]
    }

    #[test]
    fn keywords_match_case() {
        let (s, c) = complete("sel", 3, Vec::new);
        assert_eq!(s, 0);
        assert_eq!(c, vec!["select"]);
        let (_, c) = complete("SEL", 3, Vec::new);
        assert_eq!(c, vec!["SELECT"]);
        let (s, c) = complete("select * fr", 11, Vec::new);
        assert_eq!(s, 9);
        assert_eq!(c, vec!["from"]);
    }

    #[test]
    fn table_names_after_table_keywords() {
        let (s, c) = complete("select * from m", 15, tables);
        assert_eq!(s, 14);
        assert_eq!(c, vec!["movies", "music"]);
        let (_, c) = complete("SELECT * FROM ", 14, tables);
        assert_eq!(c, vec!["movies", "music", "orders"]);
        let (_, c) = complete("insert into o", 13, tables);
        assert_eq!(c, vec!["orders"]);
        // no table matches: fall back to keywords
        let (_, c) = complete("create table if no", 18, tables);
        assert_eq!(c, vec!["not", "nowait"]);
        let (_, c) = complete("drop table if exists mo", 23, tables);
        assert_eq!(c, vec!["movies"]);
        let (_, c) = complete("describe ", 9, tables);
        assert_eq!(c.len(), 3);
    }

    #[test]
    fn no_table_lookup_elsewhere() {
        let (_, c) = complete("select ", 7, || panic!("tables must not be listed"));
        assert!(c.is_empty());
        let (_, c) = complete("select * from movies where a", 28, || panic!("no"));
        assert!(c.contains(&"and".to_string()));
    }

    #[test]
    fn keyword_list_matches_lexer() {
        for k in KEYWORDS {
            assert!(crate::parser::lexer::keyword(k).is_some(), "{k}");
        }
    }
}
