# rddbsh

A Rust port of [ddbsh](https://github.com/awslabs/dynamodb-shell), a SQL-like interactive shell for Amazon DynamoDB.

> [!WARNING]
> rddbsh runs directly against your DynamoDB tables. `delete`, `drop`, `update`, scans and queries are real operations that consume capacity and may incur costs.

## Build

```sh
cargo build --release
./target/release/rddbsh
```

## Usage

```
rddbsh [-q] [-e] [-d] [-t[TERM]] [--ca-bundle <file>] [-c "<cmds>" | -f <file>]
```

| Flag | Description |
|---|---|
| `-c "<cmds>"` | Run the commands, then exit |
| `-f <file>` | Run a file, then exit |
| `-q` | Quiet: suppress the startup banner |
| `-e` | Abort on the first error |
| `-t[TERM]`, `--terminator[=TERM]` | Batch terminator (default `go`; `-t` alone = blank line) |
| `--ca-bundle <file>` | Trust a custom CA bundle (also `AWS_CA_BUNDLE` / profile `ca_bundle`) |

Interactive input follows the isql model: lines are buffered until a line containing only `go` (case-insensitive), then the batch runs.

```
$ rddbsh
ddbsh - version 0.12.1
us-east-1> create table movies (title string, year number) primary key (title hash, year range);
us-east-1> insert into movies (title, year, rating) values ("Up", 2009, 8.3);
us-east-1> select * from movies where title = "Up";
us-east-1> go
CREATE
INSERT
{rating: 8.3, title: Up, year: 2009}
```

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

The suite targets real DynamoDB (it creates and drops tables). Use `--endpoint` with `--ignore-order` for DynamoDB Local; some families (PITR, vector indexes, warm/on-demand throughput, global tables) need real DynamoDB.
