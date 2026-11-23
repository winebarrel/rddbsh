// Copyright 2026 winebarrel
// SPDX-License-Identifier: Apache-2.0
//
// Derived from ddbsh (https://github.com/awslabs/dynamodb-shell),
// Copyright Amazon.com, Inc. or its affiliates.

//! The rddbsh executable (port of cli/main.cpp): argument parsing, prompt,
//! line editing + history, isql-style input assembly, config-file loading,
//! signal handling, and wiring the wire client into the engine.

use std::fs::File;
use std::io::{BufReader, Cursor, Write};

use rddbsh::cli::{resolve_ca_bundle, run_source, LineSource, StreamLineSource, Terminator};
use rddbsh::client::DdbClient;
use rddbsh::render::Renderer;
use rddbsh::session::{ConnectionHint, Session, SessionConfig};

const USAGE: &str =
    "Usage: ddbsh [-q] [-e] [-d] [-t[TERM]] [--ca-bundle <file>] [-c \"<cmds>\" | -f <file>]\n";

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
            let candidates: Vec<&str> = ["terminator", "ca-bundle"]
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

/// Interactive stdin with line editing + history; the prompt is rebuilt from
/// the session's live connection hint before each read.
struct StdinLineSource {
    editor: rustyline::DefaultEditor,
    prompt: String,
}

impl LineSource for StdinLineSource {
    fn set_connection_hint(&mut self, hint: &ConnectionHint) {
        self.prompt = format!(
            "{}{}> ",
            hint.region,
            if hint.endpoint_overridden { " (*)" } else { "" }
        );
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
            if run_source(&mut session, &renderer, &mut src, &mut cfg_term, true) != 0 {
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
    let mut editor = match rustyline::DefaultEditor::new() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("{}", e);
            return 1;
        }
    };
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
