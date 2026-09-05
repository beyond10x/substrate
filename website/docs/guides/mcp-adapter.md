---
title: Test Substrate through MCP
description: Start one disposable daemon and let Codex or another local MCP harness exercise bounded execution through the public SDK.
---

# Give a harness a disposable Substrate

Current development source includes `substrate-mcp`, a local stdio adapter for testing Substrate
from Codex and other MCP clients. It starts one fresh private daemon, exposes a deliberately closed
set of workspace and execution tools, and removes the resources it owns when the client disconnects.

This is a development and conformance surface. It is not remote ingress, does not authenticate a
hosted principal, and does not bypass the daemon: every operation crosses the public Rust SDK and
the daemon's Unix socket.

## Build the adapter

From a source checkout:

```bash
cargo build --release --locked -p b10x-substrate-mcp --bin substrate-mcp
```

The crate is intentionally `publish = false`. Release [0.7.3](https://github.com/beyond10x/substrate/releases/tag/0.7.3) publishes the separately signed
container at
`ghcr.io/beyond10x/b10x-substrate-mcp@sha256:ac78c94823793094c7a6e46e7adfd0e4d0b83fda6c07c976893f4ff1a5fa2b8d`.
Run it with no network, a read-only root and only an ephemeral private `/tmp`:

```bash
docker run --rm -i \
  --network=none \
  --read-only \
  --tmpfs /tmp:rw,nosuid,nodev,noexec,size=16m,mode=700 \
  ghcr.io/beyond10x/b10x-substrate-mcp@sha256:ac78c94823793094c7a6e46e7adfd0e4d0b83fda6c07c976893f4ff1a5fa2b8d
```

That container remains disposable test tooling, not production ingress.

## Connect Codex

Codex supports local stdio MCP servers. Register this binary once:

```bash
codex mcp add substrate -- "$PWD/target/release/substrate-mcp"
codex mcp list
```

The Codex CLI, IDE extension, and desktop app share this MCP configuration. Restart a graphical
client after adding the server; in the terminal UI, `/mcp` shows whether it initialized.

Then ask for a concrete journey, including the limits you want:

```text
Use the substrate MCP tools. Inspect machine facts, create an empty workspace, write “hello” to
input.txt, and run ["/usr/bin/sha256sum", "/workspace/input.txt"] with 5 seconds wall time,
5 seconds CPU, 64 MiB memory, 4 processes, and 4 KiB output. Reconcile any refusal by operation ID,
show exact output and metrics when available, retire the exec, and destroy the workspace.
```

The adapter's initialization instructions remind a harness that every mutation needs a caller
operation ID. Its results carry both structured JSON and the same canonical JSON as text for MCP
clients without structured-content support.

## What the harness can do

The closed tool set covers:

- exact machine and capability facts, preserving absent facts;
- empty workspace create/get/destroy and lease renewal;
- bounded base64 file pages, writes, and deletes;
- argv-only exec start/get/wait, bounded stdout/stderr pages, signal, lease renewal, and retirement;
- durable operation lookup after an ambiguous transport result; and
- exact exec or workspace metrics when the daemon proved and served them.

Static and templated MCP resources expose the same read-only observations. PTY sessions, event and
metrics streams, snapshots, host roots, secret slots, and network apertures are deliberately absent.
In particular, a model cannot select host filesystem or network authority through this adapter.

## Portable refusal versus delegated success

The adapter never turns a missing guarantee into a weaker run. On an ordinary host without the
complete delegated Linux confinement floor, `exec_start` returns the daemon's exact named refusal,
normally `exec.sandbox-unavailable`, and its caller operation ID remains available through
`operation_get`.

For a positive native test, give one adapter an exclusive, process-free delegated cgroup v2 root:

```bash
SUBSTRATE_MCP_CGROUP_ROOT=/sys/fs/cgroup/path/to/delegated-root \
  target/release/substrate-mcp
```

The adapter takes a nonblocking owner-private lock for that exact root. A second live adapter is
refused instead of reconciling another daemon's cgroups. The repository's delegated lane creates
the right systemd scope without privilege and proves a real `sha256sum`, exact output and metrics,
then leaves a live workload for EOF cleanup and verifies both its PID and cgroup are gone:

```bash
bash scripts/delegated-lane.sh
```

## Lifecycle and limits

Each incoming or outgoing JSONL frame is capped at 2 MiB, string request IDs at 128 bytes,
concurrent calls at 16, and adapter-owned waits at 30 seconds. Stdout contains protocol frames only;
diagnostics never contain arguments, file bytes, environment values, output, or credentials.

On EOF, interrupt, or termination, the adapter stops admission, cancels waits, kills and observes
tracked live execs, retires them, destroys workspaces, and finally stops and reaps the daemon. If it
cannot prove cleanup, it exits unsuccessfully instead of reporting success. An uncatchable
`SIGKILL` still closes the daemon liveness pipe and therefore terminates the process tree, but no
claim is made that the parent can delete filesystem state after it has itself been killed.

For a manual authenticated Codex conformance run against a built binary:

```bash
cargo xtask mcp-codex-smoke --server target/release/substrate-mcp
```

That command inherits Codex's own authentication. It does not copy an API key into the adapter or
the repository, and it is intentionally not a CI correctness oracle.
