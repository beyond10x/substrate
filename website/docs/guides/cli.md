---
title: Command line
description: Inspect and drive a running Substrate daemon from a shell with the b10x-substrate client.
---

# Drive the daemon from a shell

`b10x-substrate` is a command-line client for a running daemon. It talks to the daemon's Unix
socket through the [Rust SDK](./rust-sdk.md), so it sees exactly what the SDK sees and adds no
route of its own. Use it to read the capability facts, create and remove workspaces, run a
command, and read back execs, sessions, operations, metrics and events. One verb needs no daemon:
`confine run` runs a command under a confinement adapter in its own process
([below](#run-under-a-confinement-adapter-without-a-daemon)).

## Install

b10x does not publish the crate to crates.io. From a source checkout:

```bash
cargo install --path crates/b10x-substrate-cli --locked
```

## Select the daemon

Every command except `confine run` takes `--socket <path>`. Without it, the client reads
`SUBSTRATE_SOCKET`. With neither, it exits with status 2 and names both; there is no built-in
socket path.

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
| `session signal <id> --signal int\|term\|kill` | Signals a session's process; `--grace-ms` is the grace before the whole tree is killed (default 5000) |
| `operation show <id>` | Shows an accepted operation's ledger record |
| `metrics --kind exec\|workspace --id <id>` | Shows observed resource usage |
| `events [--cursor <c>] [--limit <n>]` | Reads one page of the event log |
| `confine run [flags] -- <argv>…` | Runs a command under a confinement adapter, without a daemon |

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

A bound the daemon records on an exec that ran, such as `exec.memory-limit` or `exec.timeout`, is
an outcome of that exec and not a refusal of the request: it appears in `observation.refusal`, the
default output names its code on stderr, and the exit status stays the command's.

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

An exit status of an `exec run` passes through unchanged, so a command that itself exits 1 or 2 is
indistinguishable by status alone from a client failure; read `error.code` in JSON mode to tell
them apart.

## Capability facts

`machine` prints the facts the daemon advertises and nothing else. A fact the daemon does not
advertise is shown as `absent` in the table and is missing from the JSON `facts` object; the client
never reports it as `false` or assumes it holds.

## Run under a confinement adapter without a daemon

`confine run` opens a [confinement adapter](../concepts/adapters.md) in its own process, builds one
profile from its flags and lets the adapter admit, render or run it. It takes no socket and adds no
confinement of its own: every refusal is the adapter's named refusal or one of the verb's checks of
its flags, and each exits 2 before anything runs.

Under `linux`, `$delegated_root` is a delegated cgroup v2 directory this shell runs beneath:

```bash
export SUBSTRATE_CONFINE_CGROUP_ROOT="$delegated_root"
mkdir job
b10x-substrate confine run --dir job -- /usr/bin/printf 'hello\n'
b10x-substrate confine run --dir job --dry-run -- /usr/bin/printf 'hello\n'
```

| Flag | What it does |
|---|---|
| `--dir <path>` | the writable workspace, mounted at `/workspace`; the current directory by default. Under `linux` its name must match `[A-Za-z0-9_-]+` |
| `--ro <path>` | a host path bound read-only at the same path; repeat it, or pass a JSON list of paths. A relative path resolves `.` and `..` lexically against the current directory |
| `--net` | asks for the host network, a fact only `local` proves: under `linux` and `oci` it is refused `confinement.fact-unserved: host_network` |
| `--backend linux\|oci` | the adapter, `linux` by default |
| `--image <name>@sha256:<hex>` | the image `oci` runs, named by digest; anywhere else it is refused `confinement.fact-unserved: image_digest_pinned` |
| `--dry-run` | admits the run as a real run would, prints the program and arguments the adapter would run, one per line, and runs nothing |
| `--local <reason>` | runs unconfined under the declared-insecure `local` adapter and says `adapter=local declared_insecure=<reason>` on stderr. It needs `--net`: `local` uses the host network, so without `--net` the run asks for an isolated network `local` cannot keep, refused `confinement.family-unserved: network_egress` |

What no flag changes:

- **Bounds.** `linux` and `oci` bound every run at 600 s of wall clock, 256 processes, 1 GiB of
  memory and 600 s of CPU, and its output at the host's output limit.
- **Environment.** The command sees `PATH=/usr/bin:/bin`, `HOME=/tmp`, `LANG=C.UTF-8`,
  `LC_ALL=C.UTF-8` and `TZ=UTC`, and nothing of the caller's environment. Its standard input is
  closed, and there is no default shell: the program and its arguments after `--` are required.
- **Cgroup.** `linux` runs in a private group it makes beneath the delegated cgroup v2 root named
  by `SUBSTRATE_CONFINE_CGROUP_ROOT`, which the caller must run beneath, and removes it at exit.
  Without that variable `linux` runs nothing: it is refused
  `confinement.family-unserved: process_run`. The daemon's `SUBSTRATE_CGROUP_ROOT` is not read, so
  concurrent invocations and a daemon on the same root never reconcile each other's runs.
- **Exit status.** The command's own status, or 128 plus the signal that ended it. A refusal before
  anything runs exits 2 and a host failure 1; `--output json` tells them apart from a command that
  itself exits 1 or 2. `SIGINT` and `SIGTERM` kill the run, remove its private state directory and
  group, and exit 128 plus the signal.
