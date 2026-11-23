//! CLI input assembly (Sybase isql `go` model) and batch running
//! (port of cli/repl.{hpp,cpp}). Terminal-free so it can be unit-tested; the
//! rustyline-backed line source lives in the binary.

use std::io::{BufRead, Write};

use crate::outcome::{ExecOutcome, OutcomeSink};
use crate::render::Renderer;
use crate::session::{ConnectionHint, Session};
use crate::statement::StatementKind;

/// The batch terminator: the word `go` (case-insensitive) on its own line by
/// default, or a blank line.
#[derive(Debug, Clone, PartialEq)]
pub struct Terminator {
    /// true => a blank line ends the batch
    pub blank_line: bool,
    /// the terminator word when !blank_line
    pub word: String,
}

impl Default for Terminator {
    fn default() -> Self {
        Terminator {
            blank_line: false,
            word: "go".into(),
        }
    }
}

fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n' | '\x0c' | '\x0b')
}

fn trim(s: &str) -> &str {
    s.trim_matches(is_ws)
}

fn strip_quotes(s: &str) -> &str {
    let b = s.as_bytes();
    if b.len() >= 2 && (b[0] == b'"' || b[0] == b'\'') && b[b.len() - 1] == b[0] {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

/// CA-bundle precedence: flag > AWS_CA_BUNDLE env > profile `ca_bundle` > none.
pub fn resolve_ca_bundle(
    flag_value: Option<&str>,
    env_value: Option<&str>,
    profile_value: Option<&str>,
) -> Option<String> {
    [flag_value, env_value, profile_value]
        .into_iter()
        .flatten()
        .find(|v| !v.is_empty())
        .map(str::to_string)
}

/// Is `trimmed` (an already-trimmed line) the batch terminator?
pub fn is_terminator_line(trimmed: &str, term: &Terminator) -> bool {
    if term.blank_line {
        return trimmed.is_empty();
    }
    !trimmed.is_empty() && trimmed.eq_ignore_ascii_case(&term.word)
}

/// Handle the `set terminator [STR]` meta-command. Returns true if consumed.
pub fn try_set_terminator_directive(trimmed: &str, term: &mut Terminator) -> bool {
    let bytes = trimmed.as_bytes();
    let mut pos = 0usize;
    let next_token = |pos: &mut usize| -> &str {
        while *pos < bytes.len() && (bytes[*pos] == b' ' || bytes[*pos] == b'\t') {
            *pos += 1;
        }
        let start = *pos;
        while *pos < bytes.len() && bytes[*pos] != b' ' && bytes[*pos] != b'\t' {
            *pos += 1;
        }
        &trimmed[start..*pos]
    };
    if !next_token(&mut pos).eq_ignore_ascii_case("set") {
        return false;
    }
    if !next_token(&mut pos).eq_ignore_ascii_case("terminator") {
        return false;
    }
    let rest = strip_quotes(trim(&trimmed[pos..]));
    if rest.is_empty() {
        term.blank_line = true;
        term.word.clear();
    } else {
        term.blank_line = false;
        term.word = rest.to_string();
    }
    true
}

/// A pull source of physical input lines (None = EOF).
pub trait LineSource {
    fn next_line(&mut self) -> Option<String>;
    /// Called with the session's live connection hint before every read, so
    /// an interactive prompt can reflect a CONNECT immediately.
    fn set_connection_hint(&mut self, _hint: &ConnectionHint) {}
}

/// Lines from any buffered reader (files, `-c` strings, config files).
pub struct StreamLineSource<R: BufRead> {
    reader: R,
}

impl<R: BufRead> StreamLineSource<R> {
    pub fn new(reader: R) -> Self {
        StreamLineSource { reader }
    }
}

impl<R: BufRead> LineSource for StreamLineSource<R> {
    fn next_line(&mut self) -> Option<String> {
        let mut line = String::new();
        match self.reader.read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => {
                if line.ends_with('\n') {
                    line.pop();
                }
                Some(line)
            }
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BatchStatus {
    /// a QUIT statement ran; stop the shell
    pub quit: bool,
    /// abort_on_error fired on a failing statement
    pub aborted: bool,
    /// the LAST EXECUTED statement's error status (None when nothing ran)
    pub last_error: Option<bool>,
}

/// Writes rendered stdout/stderr bytes to the process streams.
pub struct StdioSink {
    pub renderer: Renderer,
}

impl OutcomeSink for StdioSink {
    fn emit(&mut self, fragment: ExecOutcome) {
        let out = self.renderer.render(&fragment);
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(out.as_bytes());
        let _ = stdout.flush();
        let diag = self.renderer.render_stderr(&fragment);
        if !diag.is_empty() {
            let mut stderr = std::io::stderr().lock();
            let _ = stderr.write_all(diag.as_bytes());
            let _ = stderr.flush();
        }
    }
}

fn report_error_to_stderr(rendered: &str) {
    let mut stderr = std::io::stderr().lock();
    let _ = stderr.write_all(rendered.as_bytes());
    let _ = stderr.flush();
}

/// Parse and execute one batch of text.
pub fn run_batch(
    session: &mut Session,
    renderer: &Renderer,
    text: &str,
    abort_on_error: bool,
    sink: &mut dyn OutcomeSink,
    report_error: &mut dyn FnMut(&str),
) -> BatchStatus {
    let mut status = BatchStatus::default();
    let batch = session.parse_batch(text);
    for stmt in &batch.statements {
        if let Err(e) = session.execute_streaming(stmt, sink) {
            report_error(&renderer.render_error(&e));
            status.last_error = Some(true);
            if abort_on_error {
                status.aborted = true;
                return status;
            }
            continue;
        }
        status.last_error = Some(false);
        if stmt.kind() == StatementKind::Quit {
            status.quit = true;
            return status;
        }
    }
    if let Some(e) = &batch.error {
        report_error(&renderer.render_error(e));
    }
    status
}

/// run_batch writing to the process stdout/stderr.
pub fn run_batch_text(
    session: &mut Session,
    renderer: &Renderer,
    text: &str,
    abort_on_error: bool,
) -> BatchStatus {
    let mut sink = StdioSink {
        renderer: *renderer,
    };
    run_batch(
        session,
        renderer,
        text,
        abort_on_error,
        &mut sink,
        &mut report_error_to_stderr,
    )
}

/// Drive a line source to completion writing to stdout/stderr; returns the exit code.
pub fn run_source(
    session: &mut Session,
    renderer: &Renderer,
    src: &mut dyn LineSource,
    term: &mut Terminator,
    abort_on_error: bool,
) -> i32 {
    let mut sink = StdioSink {
        renderer: *renderer,
    };
    run_source_with(
        session,
        renderer,
        src,
        term,
        abort_on_error,
        &mut sink,
        &mut report_error_to_stderr,
    )
}

/// Drive a line source: buffer lines until a terminator line, then flush the
/// batch. EOF always flushes. Returns the process exit code (1 iff the last
/// executed statement failed).
pub fn run_source_with(
    session: &mut Session,
    renderer: &Renderer,
    src: &mut dyn LineSource,
    term: &mut Terminator,
    abort_on_error: bool,
    sink: &mut dyn OutcomeSink,
    report_error: &mut dyn FnMut(&str),
) -> i32 {
    let mut buffer = String::new();
    let mut last_error: Option<bool> = None;
    let mut flush =
        |buffer: &mut String, last_error: &mut Option<bool>, session: &mut Session| -> bool {
            if buffer.is_empty() {
                return false;
            }
            let st = run_batch(
                session,
                renderer,
                buffer,
                abort_on_error,
                sink,
                report_error,
            );
            buffer.clear();
            if st.last_error.is_some() {
                *last_error = st.last_error;
            }
            st.quit || st.aborted
        };
    loop {
        src.set_connection_hint(&session.connection_hint());
        let Some(line) = src.next_line() else { break };
        let trimmed = trim(&line);
        if buffer.is_empty() && try_set_terminator_directive(trimmed, term) {
            continue;
        }
        if is_terminator_line(trimmed, term) {
            if flush(&mut buffer, &mut last_error, session) {
                return last_error.unwrap_or(false) as i32;
            }
            continue;
        }
        if !buffer.is_empty() {
            buffer.push('\n');
        }
        buffer.push_str(&line);
    }
    flush(&mut buffer, &mut last_error, session);
    last_error.unwrap_or(false) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_terminator_is_go_case_insensitive() {
        let t = Terminator::default();
        assert!(is_terminator_line("go", &t));
        assert!(is_terminator_line("GO", &t));
        assert!(!is_terminator_line("", &t));
        assert!(!is_terminator_line("gone", &t));
    }

    #[test]
    fn blank_line_terminator() {
        let t = Terminator {
            blank_line: true,
            word: String::new(),
        };
        assert!(is_terminator_line("", &t));
        assert!(!is_terminator_line("go", &t));
    }

    #[test]
    fn set_terminator_directive() {
        let mut t = Terminator::default();
        assert!(try_set_terminator_directive(
            "SET terminator  'run'",
            &mut t
        ));
        assert_eq!(
            t,
            Terminator {
                blank_line: false,
                word: "run".into()
            }
        );
        assert!(try_set_terminator_directive("set terminator", &mut t));
        assert!(t.blank_line);
        assert!(!try_set_terminator_directive("set x", &mut t));
        assert!(!try_set_terminator_directive("select * from t", &mut t));
    }

    #[test]
    fn ca_bundle_precedence() {
        assert_eq!(
            resolve_ca_bundle(Some("f"), Some("e"), Some("p")),
            Some("f".into())
        );
        assert_eq!(
            resolve_ca_bundle(Some(""), Some("e"), Some("p")),
            Some("e".into())
        );
        assert_eq!(resolve_ca_bundle(None, None, Some("p")), Some("p".into()));
        assert_eq!(resolve_ca_bundle(None, Some(""), None), None);
    }

    #[test]
    fn stream_line_source_strips_newlines() {
        let mut src = StreamLineSource::new("a\nb\n\nc".as_bytes());
        let mut lines = vec![];
        while let Some(l) = src.next_line() {
            lines.push(l);
        }
        assert_eq!(lines, vec!["a", "b", "", "c"]);
    }
}
