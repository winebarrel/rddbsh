# rddbsh

A Rust port of [ddbsh](https://github.com/awslabs/dynamodb-shell), a SQL-like interactive shell for Amazon DynamoDB.

> [!WARNING]
> rddbsh runs directly against your DynamoDB tables. `delete`, `drop`, `update`, scans and queries are real operations that consume capacity and may incur costs.

> [!TIP]
> With `--immediate`, statements run when a line ends with `;`, without `go`. See [Immediate mode](#immediate-mode---immediate).

## Install

```sh
brew install winebarrel/rddbsh/rddbsh
```

Prebuilt archives for macOS (Apple Silicon), Linux (x86_64, aarch64) and Windows (x86_64) are on the [releases page](https://github.com/winebarrel/rddbsh/releases).

## Build

```sh
cargo build --release
./target/release/rddbsh
```

## Usage

```
rddbsh [-q] [-e] [-d] [-t[TERM]] [--immediate] [--ca-bundle <file>] [--show-retries]
       [--output default|json|csv|table] [--json-flavor plain|dynamodb] [-c "<cmds>" | -f <file>]
```

| Flag | Description |
|---|---|
| `-c "<cmds>"` | Run the commands, then exit |
| `-f <file>` | Run a file, then exit |
| `-q` | Quiet: suppress the startup banner |
| `-e` | Abort on the first error |
| `-t[TERM]`, `--terminator[=TERM]` | Batch terminator (default `go`; `-t` alone = blank line) |
| `--immediate` | Run each statement as soon as it ends with `;` (no `go` needed) |
| `--ca-bundle <file>` | Trust a custom CA bundle (also `AWS_CA_BUNDLE` / profile `ca_bundle`) |
| `--show-retries` | Print a line to stderr for each retry (throttling or a transient error), before its backoff |
| `--output <format>` | Output format for rows: `default`, `json`, `csv` or `table`. See [Output formats](#output-formats) |
| `--json-flavor <flavor>` | How JSON writes values: `plain` (default) or `dynamodb` (`{"N": "1"}`) |

Interactive input follows the isql model: lines are buffered until a line containing only `go` (case-insensitive), then the batch runs.

```
$ rddbsh
ddbsh - version x.y.z
us-east-1> create table movies (title string, year number) primary key (title hash, year range);
us-east-1> insert into movies (title, year, rating) values ("Up", 2009, 8.3);
us-east-1> select * from movies where title = "Up";
us-east-1> go
CREATE
INSERT
{rating: 8.3, title: Up, year: 2009}
```

### Immediate mode (`--immediate`)

With `--immediate`, you don't need `go`: a statement runs as soon as a line ends with `;`, like the MySQL and PostgreSQL clients.

```
$ rddbsh --immediate
ddbsh - version x.y.z
us-east-1> select * from movies where title = "Up";
{rating: 8.3, title: Up, year: 2009}
us-east-1> select *
us-east-1>   from movies;
{rating: 8.3, title: Up, year: 2009}
```

- A statement can span several lines; nothing runs until the `;`.
- A `;` inside a string or a comment does not run anything.
- A transaction (`begin; ... commit;` or `rollback;`) runs as one unit once `commit` or `rollback` is reached.
- A `go` line still works.
- To switch modes inside the shell, use `set immediate on` or `set immediate off`. To make immediate mode the default, put `set immediate on` in `~/.ddbsh_config`.

### Output formats

`--output json|csv|table` changes how rows are printed: the items of a `SELECT`, a read transaction and `RETURN VALUES`, and the rows of `SHOW TABLES`, `SHOW BACKUPS` and `SHOW LIMITS`. Status lines, `DESCRIBE`, `EXPLAIN` and errors stay text.

```
$ rddbsh -q --output table -c 'select * from movies where title = "Up"'
+-------+------+--------+
| title | year | rating |
+-------+------+--------+
| Up    | 2009 |    8.3 |
+-------+------+--------+
```

- `json` prints one JSON array per result set. `--json-flavor dynamodb` writes the DynamoDB typed encoding (`{"N": "8.3"}`) instead of plain values.
- `csv` prints a header, then one line per row (RFC 4180 quoting).
- `table` prints a text table per API page.
- `set output <format>` and `set json-flavor <flavor>` on their own line at the start of a batch change the format mid-session. In `~/.ddbsh_config` they set the default; the flags override them.

### Tab completion

Press <kbd>Tab</kbd> to complete keywords and table names. Table names are offered after `from`, `into`, `update`, `describe`, `table` and similar keywords. They come from `ListTables` and are cached for 30 seconds.

Credentials and region come from the standard AWS chain (environment, profiles, SSO, IMDS).

| Environment variable | Description |
|---|---|
| `AWS_REGION` / `AWS_DEFAULT_REGION` | Region (default `us-east-1`) |
| `DDBSH_ENDPOINT_OVERRIDE` | Endpoint URL (e.g. `http://localhost:8000` for DynamoDB Local) |
| `DDBSH_CONFIG_FILE` | Startup command file (default `~/.ddbsh_config`) |

Run `help;` in the shell for the full command reference.

## Architecture

- **Engine** (`src/`): lexer/parser, WHERE expression tree, GetItem/Query/Scan planner, executors and renderer. It reaches DynamoDB only through the `DdbClient` trait.
- **Wire client** (`src/wire/`): serializes requests to DynamoDB JSON, signs them with SigV4, and sends them over HTTPS. EXPLAIN prints exactly the body that would be sent.
- **CLI** (`src/cli.rs`, `src/bin/rddbsh.rs`): argument parsing, line editing (rustyline), history, and batch input.

The web (WebAssembly) edition of ddbsh is not ported.

## Testing

```sh
cargo test
```

`tests/golden/` contains ddbsh's golden suite, unchanged. `run-golden.py` runs it against the built binary:

```sh
tests/golden/run-golden.py --bin target/debug/rddbsh [--family select] [--endpoint URL] [--ignore-order]
```

`ifmt-bad-output` and `ifmt-bad-flavor` are expected to fail: their goldens hold ddbsh's usage line, and rddbsh's also lists `--immediate`.

The suite targets real DynamoDB (it creates and drops tables). Use `--endpoint` with `--ignore-order` for DynamoDB Local; some families (PITR, vector indexes, warm/on-demand throughput, global tables) need real DynamoDB.

## Releasing

Releases are built by [dist](https://opensource.axo.dev/cargo-dist/) (`.github/workflows/release.yml`). Bump `version` in `Cargo.toml`, then push a matching tag:

```sh
git tag vX.Y.Z && git push origin vX.Y.Z
```

The workflow uploads archives (macOS Apple Silicon, Linux, Windows) with checksums to GitHub Releases and updates the Homebrew formula.

## License

Apache License 2.0. rddbsh is a derivative work of [ddbsh](https://github.com/awslabs/dynamodb-shell) (Copyright Amazon.com, Inc. or its affiliates). See [LICENSE](LICENSE) and [NOTICE](NOTICE).

rddbsh is not affiliated with or endorsed by Amazon Web Services.
