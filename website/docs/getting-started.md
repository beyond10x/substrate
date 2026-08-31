---
title: Getting started
description: Build and run a personal Substrate daemon over an owner-permissioned Unix socket.
---

# Start with an observable machine

The safest first run uses the personal deployment over a Unix socket. It lets you inspect machine
facts and exercise workspace operations without implying that this host can safely execute a
process.

## Prerequisites

- Linux
- Rust 1.97 or newer
- `curl`
- `jq` for readable JSON output

Bubblewrap, `/usr/bin/socat`, and a delegated cgroup v2 subtree are additional requirements for
process execution. The probe uses `socat` to prove that a sandbox cannot reach the host's Unix
socket namespace. They are not needed merely to start the daemon and inspect its facts.

## Build the workspace

From a Substrate checkout:

```bash
cargo build --workspace --locked
```

## Run the daemon

Create a private runtime directory and start the daemon with your numeric user ID explicitly
allowed:

```bash
mkdir -p ./run
target/debug/substrate-daemon \
  --socket ./run/substrate.sock \
  --state ./run/state.db \
  --workspaces ./run/workspaces \
  --deployment personal \
  --event-retention 10000 \
  --allow-uid "$(id -u)"
```

The daemon derives the subject from kernel peer credentials on each Unix-socket connection. There
is no request header or JSON field that can choose a subject.

## Inspect verified facts

In another terminal:

```bash
curl --silent --show-error \
  --unix-socket ./run/substrate.sock \
  http://localhost/v1/machine | jq
```

Read the capability document literally. If execution confinement is unavailable, its facts remain
absent. A later exec request is refused with a named `exec.sandbox-unavailable` outcome; Substrate
does not fall back to an ordinary host process.

## What to look for

The response describes the deployment, contract identity, driver generation, verified capabilities,
and operational limits. These are observations about this running daemon, not promises inferred from
its configuration.

## Serving process execution

A Linux host that serves exec needs all of the following:

1. bubblewrap available at the configured path;
2. user, mount, PID, IPC, UTS, and network namespaces;
3. a delegated cgroup v2 subtree with `cpu`, `memory`, and `pids` controllers;
4. a process-free delegation root;
5. `/usr/bin/socat` for the host-IPC confinement probe;
6. the delegation passed with `--cgroup-root`.

The daemon probes the backend before advertising execution. Missing enforcement produces a refusal,
not weaker isolation.

Continue with [run a bounded command](./guides/run-a-command.md) for a complete terminal journey.
Read [confinement and refusal](./concepts/confinement.md) before admitting process work, or
[deployment postures](./guides/deployment.md) before making the daemon reachable beyond its owner.
