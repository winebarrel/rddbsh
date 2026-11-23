// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! The rddbsh executable (port of cli/main.cpp): argument parsing, prompt,
//! line editing + history, isql-style input assembly, config-file loading,
//! signal handling, and wiring the wire client into the engine.

use std::cell::RefCell;
use std::fs::File;
use std::io::{BufReader, Cursor, Write};
use std::time::{Duration, Instant};

use rustyline::completion::{Completer, Pair};
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::history::DefaultHistory;
use rustyline::validate::Validator;
use rustyline::{Context, Editor, Helper};

use rddbsh::cli::{resolve_ca_bundle, run_source, LineSource, StreamLineSource, Terminator};
use rddbsh::client::{DdbClient, ListTablesIn};
use rddbsh::completion;
use rddbsh::render::Renderer;
use rddbsh::session::{ConnectionHint, Session, SessionConfig};

const USAGE: &str =
    "Usage: ddbsh [-q] [-e] [-d] [-t[TERM]] [--immediate] [--ca-bundle <file>] [-c \"<cmds>\" | -f <file>]\n";

fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

#[derive(Default)]
struct Options {
    quiet: bool,
    abort_on_error: bool,
    debug: bool,
    cmdstring: Option<String>,
    inputfile: Option<String>,
    ca_bundle: Option<String>,
    term: Terminator,
}

fn set_terminator(term: &mut Terminator, arg: Option<&str>) {
    match arg {
        Some(a) if !a.is_empty() => {
            term.blank_line = false;
            term.word = a.to_string();
        }
        _ => {
            term.blank_line = true;
            term.word.clear();
        }
    }
}

/// getopt_long-compatible parsing of "qc:f:edt::" plus --terminator[=STR]
/// and --ca-bundle <file>. Non-option arguments are ignored (GNU permutes
/// them to the end; the original never reads them).
fn parse_args(prog: &str, args: &[String]) -> Result<Options, ()> {
    let mut o = Options::default();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        if arg == "--" {
            break;
        }
        if let Some(long) = arg.strip_prefix("--") {
            let (name, value) = match long.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (long, None),
            };
            let candidates: Vec<&str> = ["terminator", "immediate", "ca-bundle"]
                .into_iter()
                .filter(|c| !name.is_empty() && c.starts_with(name))
                .collect();
            let full = match candidates.as_slice() {
                [one] => *one,
                [] => {
                    eprintln!("{}: unrecognized option '--{}'", prog, name);
                    return Err(());
                }
                _ => {
                    eprintln!("{}: option '--{}' is ambiguous", prog, name);
                    return Err(());
                }
            };
            if full == "terminator" {
                set_terminator(&mut o.term, value.as_deref());
            } else if full == "immediate" {
                if value.is_some() {
                    eprintln!("{}: option '--immediate' doesn't allow an argument", prog);
                    return Err(());
                }
                o.term.immediate = true;
            } else {
                let v = match value {
                    Some(v) => v,
                    None if i < args.len() => {
                        i += 1;
                        args[i - 1].clone()
                    }
                    None => {
                        eprintln!("{}: option '--ca-bundle' requires an argument", prog);
                        return Err(());
                    }
                };
                o.ca_bundle = Some(v);
            }
            continue;
        }
        let Some(flags) = arg.strip_prefix('-') else {
            continue;
        };
        if flags.is_empty() {
            continue; // a lone "-" is a non-option argument
        }
        let chars: Vec<char> = flags.chars().collect();
        let mut k = 0;
        while k < chars.len() {
            let c = chars[k];
            k += 1;
            let rest: String = chars[k..].iter().collect();
            match c {
                'q' => o.quiet = true,
                'e' => o.abort_on_error = true,
                'd' => o.debug = true,
                't' => {
                    set_terminator(
                        &mut o.term,
                        if rest.is_empty() { None } else { Some(&rest) },
                    );
                    break;
                }
                'c' | 'f' => {
                    let v = if !rest.is_empty() {
                        rest
                    } else if i < args.len() {
                        i += 1;
                        args[i - 1].clone()
                    } else {
                        eprintln!("{}: option requires an argument -- '{}'", prog, c);
                        return Err(());
                    };
                    if c == 'c' {
                        o.cmdstring = Some(v);
                    } else {
                        o.inputfile = Some(v);
                    }
                    break;
                }
                other => {
                    eprintln!("{}: invalid option -- '{}'", prog, other);
                    return Err(());
                }
            }
        }
    }
    Ok(o)
}

fn config_path() -> Option<String> {
    if let Some(p) = env_nonempty("DDBSH_CONFIG_FILE") {
        return Some(p);
    }
    env_nonempty("HOME").map(|h| format!("{}/.ddbsh_config", h))
}

fn history_path() -> Option<String> {
    env_nonempty("HOME").map(|h| format!("{}/.ddbsh_history", h))
}

/// strerror-style message for an io::Error (drops Rust's " (os error N)").
fn os_message(e: &std::io::Error) -> String {
    let s = e.to_string();
    match s.find(" (os error") {
        Some(ix) => s[..ix].to_string(),
        None => s,
    }
}

/// How long listed table names are reused for completion.
const TABLE_CACHE_TTL: Duration = Duration::from_secs(30);

/// Tab completion: keywords, plus table names where a table is expected.
/// Table names come from ListTables on a client of its own (the session's
/// client is busy running the REPL), cached per region/endpoint.
struct CompletionHelper {
    region: String,
    endpoint: String,
    ca_bundle: Option<String>,
    state: RefCell<TableCache>,
}

#[derive(Default)]
struct TableCache {
    client: Option<((String, String), Box<dyn DdbClient>)>,
    tables: Option<((String, String), Instant, Vec<String>)>,
}

impl CompletionHelper {
    fn table_names(&self) -> Vec<String> {
        let key = (self.region.clone(), self.endpoint.clone());
        let mut st = self.state.borrow_mut();
        if let Some((k, at, names)) = &st.tables {
            if *k == key && at.elapsed() < TABLE_CACHE_TTL {
                return names.clone();
            }
        }
        if st.client.as_ref().is_none_or(|(k, _)| *k != key) {
            match open_client(&self.region, &self.endpoint, self.ca_bundle.clone()) {
                Ok(c) => st.client = Some((key.clone(), c)),
                Err(_) => return Vec::new(),
            }
        }
        let Some((_, client)) = st.client.as_mut() else {
            return Vec::new();
        };
        let names = client
            .list_tables(&ListTablesIn::default())
            .map(|o| o.table_names)
            .unwrap_or_default();
        st.tables = Some((key, Instant::now(), names.clone()));
        names
    }
}

impl Completer for CompletionHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let (start, words) = completion::complete(line, pos, || self.table_names());
        let pairs = words
            .into_iter()
            .map(|w| Pair {
                display: w.clone(),
                replacement: w,
            })
            .collect();
        Ok((start, pairs))
    }
}

impl Hinter for CompletionHelper {
    type Hint = String;
}
impl Highlighter for CompletionHelper {}
impl Validator for CompletionHelper {}
impl Helper for CompletionHelper {}

/// Interactive stdin with line editing + history; the prompt is rebuilt from
/// the session's live connection hint before each read.
struct StdinLineSource {
    editor: Editor<CompletionHelper, DefaultHistory>,
    prompt: String,
}

impl LineSource for StdinLineSource {
    fn set_connection_hint(&mut self, hint: &ConnectionHint) {
        self.prompt = format!(
            "{}{}> ",
            hint.region,
            if hint.endpoint_overridden { " (*)" } else { "" }
        );
        if let Some(h) = self.editor.helper_mut() {
            h.region = hint.region.clone();
            h.endpoint = hint.endpoint.clone();
        }
    }

    fn next_line(&mut self) -> Option<String> {
        loop {
            match self.editor.readline(&self.prompt) {
                Ok(line) => {
                    if !line.is_empty() {
                        let _ = self.editor.add_history_entry(line.as_str());
                    }
                    return Some(line);
                }
                // Ctrl-C interrupts the current prompt without killing the shell.
                Err(rustyline::error::ReadlineError::Interrupted) => continue,
                Err(_) => return None,
            }
        }
    }
}

fn open_client(
    region: &str,
    endpoint: &str,
    ca_bundle: Option<String>,
) -> Result<Box<dyn DdbClient>, String> {
    rddbsh::wire::open(region, endpoint, ca_bundle)
        .map(|c| Box::new(c) as Box<dyn DdbClient>)
        .map_err(|e| e.message)
}

fn main() {
    std::process::exit(real_main());
}

fn real_main() -> i32 {
    // A no-op SIGINT handler: Ctrl-C does not kill the shell.
    let _ = ctrlc::set_handler(|| {});

    let args: Vec<String> = std::env::args().collect();
    let prog = args
        .first()
        .map(|p| p.rsplit('/').next().unwrap_or(p).to_string())
        .unwrap_or_else(|| "ddbsh".into());
    let mut opts = match parse_args(&prog, &args[1..]) {
        Ok(o) => o,
        Err(()) => {
            eprint!("{}", USAGE);
            return 1;
        }
    };
    let _ = opts.debug; // accepted for flag-surface parity; no logging subsystem

    if opts.cmdstring.is_some() && opts.inputfile.is_some() {
        println!("ddbsh: Cannot specify both -c and -f.");
        let _ = std::io::stdout().flush();
        return 1;
    }

    let region = env_nonempty("AWS_REGION")
        .or_else(|| env_nonempty("AWS_DEFAULT_REGION"))
        .unwrap_or_else(|| "us-east-1".into());
    let endpoint = env_nonempty("DDBSH_ENDPOINT_OVERRIDE").unwrap_or_default();

    let profile_ca = rddbsh::wire::profile_ca_bundle();
    let env_ca = env_nonempty("AWS_CA_BUNDLE");
    let ca_bundle = resolve_ca_bundle(
        opts.ca_bundle.as_deref(),
        env_ca.as_deref(),
        profile_ca.as_deref(),
    );

    let completion_ca_bundle = ca_bundle.clone();
    let client = match open_client(&region, &endpoint, ca_bundle) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("{}", msg);
            return 1;
        }
    };
    let mut session = Session::new(
        SessionConfig {
            region: region.clone(),
            endpoint: endpoint.clone(),
        },
        client,
    );
    let renderer = Renderer;

    // Config file: abort-on-error so a bad line stops startup; a QUIT there
    // ends config processing only.
    if let Some(cfg) = config_path() {
        if let Ok(f) = File::open(&cfg) {
            let mut cfg_term = Terminator::default();
            let mut src = StreamLineSource::new(BufReader::new(f));
            let rv = run_source(&mut session, &renderer, &mut src, &mut cfg_term, true);
            // `set immediate on` in the config file turns the mode on for the session.
            opts.term.immediate |= cfg_term.immediate;
            if rv != 0 {
                return 1;
            }
        }
    }

    if !opts.quiet {
        println!("ddbsh - version {}", rddbsh::version());
        let _ = std::io::stdout().flush();
    }

    if let Some(cmd) = opts.cmdstring.take() {
        let mut src = StreamLineSource::new(Cursor::new(cmd.into_bytes()));
        return run_source(
            &mut session,
            &renderer,
            &mut src,
            &mut opts.term,
            opts.abort_on_error,
        );
    }

    if let Some(path) = opts.inputfile.take() {
        let f = match File::open(&path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("{}: {}", path, os_message(&e));
                return 1;
            }
        };
        let mut src = StreamLineSource::new(BufReader::new(f));
        return run_source(
            &mut session,
            &renderer,
            &mut src,
            &mut opts.term,
            opts.abort_on_error,
        );
    }

    // Interactive REPL.
    let mut editor = match Editor::<CompletionHelper, DefaultHistory>::new() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("{}", e);
            return 1;
        }
    };
    editor.set_helper(Some(CompletionHelper {
        region: region.clone(),
        endpoint: endpoint.clone(),
        ca_bundle: completion_ca_bundle,
        state: RefCell::new(TableCache::default()),
    }));
    let history = history_path();
    if let Some(h) = &history {
        let _ = editor.load_history(h);
    }
    let mut src = StdinLineSource {
        editor,
        prompt: String::new(),
    };
    let rv = run_source(
        &mut session,
        &renderer,
        &mut src,
        &mut opts.term,
        opts.abort_on_error,
    );
    if let Some(h) = &history {
        let _ = src.editor.save_history(h);
    }
    rv
}
