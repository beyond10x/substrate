---
title: Command line
description: Inspect and drive a running Substrate daemon from a shell with the b10x-substrate client.
---

# Drive the daemon from a shell

`b10x-substrate` is a command-line client for a running daemon. It talks to the daemon's Unix
socket through the [Rust SDK](./rust-sdk.md), so it sees exactly what the SDK sees and adds no
route of its own. Use it to read the capability facts, create and remove workspaces, run a
command, and read back execs, sessions, operations, metrics and events.

One verb needs no daemon: `confine run` runs a command under a confinement adapter in its own
process ([below](#run-a-command-under-confinement-without-a-daemon)).

## Install

b10x does not publish the crate to crates.io. From a source checkout:

```bash
cargo install --path crates/b10x-substrate-cli --locked
```

## Select the daemon

Every command takes `--socket <path>`. Without it, the client reads `SUBSTRATE_SOCKET`. With
neither, it exits with status 2 and names both; there is no built-in socket path. `confine run`
reads neither.

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

## Run a command under confinement without a daemon

`confine run` builds one confinement profile from its flags, opens one adapter of the host crate's
[confinement API](./confinement-profiles.md) in its own process, and has that adapter admit and run
the command. It adds no confinement of its own. Every refusal is the adapter's named refusal or one
of the verb's checks of its own flags, and each one exits 2 before anything runs.

```bash
export SUBSTRATE_CONFINE_CGROUP_ROOT=/sys/fs/cgroup/<delegated-root>
b10x-substrate confine run --dir ./ws --ro ../toolchain -- /bin/sh -c 'echo built > out'
```

| Flag | What it does |
|---|---|
| `--dir <path>` | The writable workspace; the current directory when absent. `linux` and `oci` mount it at `/workspace`. Under `linux` its name must match `[A-Za-z0-9_-]+` |
| `--ro <path>` | A host path bound read-only at the same path. Repeat it, or pass a JSON list of paths. A relative path is made absolute against the current directory, with `.` and `..` resolved lexically and symbolic links left in place |
| `--net [true\|false]` | Asks for the host network; `--net` alone means `true`. Without it the run has no network |
| `--backend linux\|oci` | The adapter; `linux` when absent |
| `--image <name>@sha256:<64 hex>` | The image `oci` runs |
| `--dry-run [true\|false]` | Admits the run as a real run would, prints the program and arguments the adapter would run, one per line, and runs nothing |
| `--local <reason>` | Runs the command unconfined under the declared-insecure `local` adapter, giving the reason; it takes precedence over `--backend` |

The program and its arguments after `--` are required; there is no default shell. The command's
standard input is closed. Its environment is `PATH=/usr/bin:/bin`, `HOME=/tmp`, `LANG=C.UTF-8`,
`LC_ALL=C.UTF-8` and `TZ=UTC`, and nothing of the calling process's environment. `linux` and `oci`
bound every run at 600 s of wall clock, 256 processes, 1 GiB of memory and 600 s of CPU; no flag
changes them. Each invocation keeps its state in a private directory, mode 0700, removed when it
exits.

### The adapters

`linux` is the host driver: bubblewrap, namespaces, cgroup v2 and seccomp. It runs only beneath the
delegated cgroup v2 root that `SUBSTRATE_CONFINE_CGROUP_ROOT` names, and this process must run
beneath that root, not directly in it. The daemon's `SUBSTRATE_CGROUP_ROOT` is not read. Without
the variable, `linux` runs nothing: the run is refused `confinement.family-unserved: process_run`.

Each invocation makes its own group, `confine-<pid>-<start time>`, beneath that root, moves itself
into a leaf of it and opens the driver on that group, so two invocations, or one beside a daemon on
the same root, never reconcile each other's runs. At exit it moves back, kills what is left in the
group and removes it. A root where no such group can be made is refused `exec.sandbox-unavailable`,
naming the variable.

`SIGKILL` skips that removal. The next invocation on the same root removes each `confine-` group
whose owner is gone (no process with that pid, or one started at another time) and that holds no
process. It never kills: a dead owner's group that still holds a process is left in place. Each
group it leaves or cannot remove, and a root it cannot read, is named on one `b10x-substrate:` line
on stderr, a plain text line under `--output json` too; none of them changes the invocation's own
outcome.

`oci` runs one `docker` container per run, `docker` found on `PATH`, with the image named by sha256
digest only.

`--local <reason>` runs an unconfined host process. In the default output, stderr says
`b10x-substrate: adapter=local declared_insecure=<reason>`; the JSON result carries the reason in
`declared_insecure`. The host's operator forbids `local` by setting `SUBSTRATE_CONFINEMENT_LOCAL`
to any value other than `permit`.

### Refusals

| When | Refusal |
|---|---|
| `--net` under `linux` or `oci` | `confinement.fact-unserved: host_network` |
| `--local` without `--net` (`local` cannot withhold the host network) | `confinement.family-unserved: network_egress` |
| `--ro` with `--local` | `confinement.family-unserved: filesystem_view` |
| `--image` without `--backend oci`, or with `--local` | `confinement.fact-unserved: image_digest_pinned` |
| `--backend oci` without `--image`, or with an image named by tag | `confinement.image-unpinned` |
| `linux` without `SUBSTRATE_CONFINE_CGROUP_ROOT` | `confinement.family-unserved: process_run` |
| a cgroup root where no private group can be made | `exec.sandbox-unavailable` |
| `--local` with a blank reason | `confinement.local-reason-missing` |
| `--local` where the host's policy forbids it | `confinement.local-refused` |
| an adapter that does not open on this host | `confinement.adapter-unserved` |

A `--dir` whose name breaks the `linux` rule is a `cli_input` error that names the rule. A dry run
meets the same admission as a run, so it is refused wherever the run would be.

### Exit status and output

The verb exits with the command's own status, or 128 plus the number of the signal that ended it.
A run that ended without an exit status exits 1. A refusal exits 2 and a host failure while
running exits 1, so a command that itself exits 1 or 2 is told apart only by the output;
`--output json` separates them. `SIGINT` or `SIGTERM` sent to the verb kills the run, removes its
state directory and private group, and exits 128 plus that signal, as `confine.interrupted`.

In the default output, the command's stdout and stderr are written once it has ended. A stream cut
at the host's output limit, and a run that ended without an exit status, are each named on stderr.
A refusal is one line on stderr, `b10x-substrate: <code>: <message>`. A dry run prints the program
and its arguments on stdout and, on stderr, that descriptor numbers in them name nothing once the
render returns.

With `--output json`, a run prints one document on stdout, `{"ok":true,"result":…}`. The result
carries `adapter`, `declared_insecure`, `exit_code`, `signal`, `stdout`, `stderr`,
`stdout_truncated` and `stderr_truncated`, with stdout and stderr as text and any invalid UTF-8
replaced. A dry run's result carries `adapter`, `declared_insecure`, `program`, `args` and `note`.
A failure is one document on stderr, `{"ok":false,"error":{"code":…,"data":…}}`:

| `error.code` | `data` | Exit |
|---|---|---|
| `refused` | `code`, `message`, `status` and `address` of the named refusal | 2 |
| `failed` | the same, for a host failure while running, or `confine.interrupted` | 1, or 128 plus the signal |
| `cli_input` | `message` | 2 |

Unlike the daemon commands, a refusal here exits 2.
