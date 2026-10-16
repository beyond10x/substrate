---
title: Command line
description: Inspect and drive a running Substrate daemon from a shell with the b10x-substrate client.
---

# Drive the daemon from a shell

`b10x-substrate` is a command-line client for a running daemon. It talks to the daemon's Unix
socket through the [Rust SDK](./rust-sdk.md), so it sees exactly what the SDK sees and adds no
route of its own. Use it to read the capability facts, create and remove workspaces, run a
command, and read back execs, sessions, operations, metrics and events.

## Install

b10x does not publish the crate to crates.io. From a source checkout:

```bash
cargo install --path crates/b10x-substrate-cli --locked
```

## Select the daemon

Every command takes `--socket <path>`. Without it, the client reads `SUBSTRATE_SOCKET`. With
neither, it exits with status 2 and names both; there is no built-in socket path.

```bash
export SUBSTRATE_SOCKET=./run/substrate.sock
b10x-substrate machine
```

## Commands

| Command | What it does |
|---|---|
| `machine` | Shows the capability snapshot and every fact the daemon advertises |
| `workspace create` | Creates an empty workspace (`--labels`, `--storage`, `--lease-ttl-ms`) |
| `workspace show <id>` | Shows a workspace |
| `workspace renew <id> --ttl-ms <ms>` | Renews a workspace lease |
| `workspace destroy <id>` | Destroys a workspace |
| `exec run --workspace <id> -- <argv>…` | Runs a command and waits for it to end |
| `exec show <id>` | Shows an exec |
| `session show <id>` | Shows a session |
| `session signal <id> --signal int\|term\|kill` | Signals a session's process |
| `operation show <id>` | Shows an accepted operation's ledger record |
| `metrics --kind exec\|workspace --id <id>` | Shows observed resource usage |
| `events [--cursor <c>] [--limit <n>]` | Reads one page of the event log |

`--labels` and `--storage` take JSON, for example `--labels '{"team":"a"}'` and
`--storage '{"max_bytes":1048576,"max_inodes":1024}'`. Mutating commands take `--operation-id`, a
client-chosen idempotency key.

## Run a command

```bash
ws=$(b10x-substrate --output json workspace create | jq -r .result.workspace)
b10x-substrate exec run --workspace "$ws" -- /usr/bin/printf 'hello\n'
```

In the default output, the command's stdout and stderr bytes pass through unchanged, and the
client exits with the command's exit code, or 128 plus the signal number when a signal ended it.
The execution limits default to 30 s wall clock, 30 s CPU, 256 MiB memory, 64 processes and 1 MiB
of recorded output per stream; `--timeout-ms`, `--cpu-millis`, `--memory-bytes`, `--processes` and
`--output-bytes` change them.

On a host where the daemon cannot confine a process, `exec run` prints the daemon's named refusal,
`exec.sandbox-unavailable`, and exits non-zero. It never runs the command unconfined.

## Machine-readable output

`--output json` prints one JSON document per call. A success goes to stdout as
`{"ok":true,"result":…}`; every result carries the daemon's observation in `observation`. A failure
goes to stderr as `{"ok":false,"error":{"code":…,"data":…}}`:

| `error.code` | Meaning | Exit |
|---|---|---|
| `refused` | The daemon refused; `data.code` is its named code, such as `resource.not-found` | 1 |
| `unavailable` | The daemon could not be reached or answered outside the contract; `data.reason` says which | 1 |
| `cli_input` | The invocation is unusable, such as no socket selected | 2 |

`exec run` in JSON mode returns `stdout` and `stderr` as text, with any invalid UTF-8 replaced, and
still exits with the command's own status.

## Capability facts

`machine` prints the facts the daemon advertises and nothing else. A fact the daemon does not
advertise is shown as `absent` in the table and is missing from the JSON `facts` object; the client
never reports it as `false` or assumes it holds.
