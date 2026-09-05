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
install -d -m 700 ./run
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
curl --silent --show-error --fail-with-body --max-time 30 \
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

## Try a workspace without process execution

This first round trip creates workspace W1, writes the input used by the command guide, reads it
back, and destroys the workspace. It needs guarded workspace I/O; it does not request an exec or a
hard disk quota. Run this as a Bash script from the checkout, with the daemon still running.

The helper fails on transport, HTTP and response-envelope errors. A fresh run gets fresh operation
IDs; keep the printed run ID and request bodies if a connection fails, so you can reconcile the
same operations instead of repeating effects.

```bash
set -euo pipefail
SOCKET=./run/substrate.sock
BASE=http://localhost
RUN_ID=$(cat /proc/sys/kernel/random/uuid)
printf 'run: %s\n' "$RUN_ID"

api() {
  local response
  response=$(curl --silent --show-error --fail-with-body --max-time 30 \
    --unix-socket "$SOCKET" "$@") || { printf '%s\n' "$response" >&2; return 1; }
  jq -e 'if .error == null and .result != null then . else error("API failure") end' \
    <<<"$response"
}

CREATE_BODY=$(jq -nc --arg op "$RUN_ID-create" \
  '{op: $op, input: {source: "empty", labels: {purpose: "handbook"}}}')
CREATE=$(api --header 'content-type: application/json' \
  --data "$CREATE_BODY" "$BASE/v1/workspaces")
WS=$(jq -er '.result.id | select(type == "string" and length > 0)' <<<"$CREATE")
printf 'workspace: %s\n' "$WS"

CONTENT=$(printf '%s' 'substrate runs ordinary binaries' | base64 -w0)
WRITE_BODY=$(jq -nc --arg op "$RUN_ID-write" --arg data "$CONTENT" \
  '{op: $op, input: {content: {encoding: "base64", data: $data}}}')
api --request PUT --header 'content-type: application/json' \
  --data "$WRITE_BODY" "$BASE/v1/workspaces/$WS/files/input.txt"

api "$BASE/v1/workspaces/$WS/files/input.txt?mode=file&offset=0&limit_bytes=4096" |
  jq -er '.result.content.data | select(type == "string")' | base64 -d
printf '\n'

DESTROY_BODY=$(jq -nc --arg op "$RUN_ID-destroy" '{op: $op, input: {}}')
api --request DELETE --header 'content-type: application/json' \
  --data "$DESTROY_BODY" "$BASE/v1/workspaces/$WS"
```

If a mutation loses its response, inspect `GET /v1/ops/{operation_id}` before making another
attempt. If the script stops after workspace creation, its printed workspace ID remains available
for inspection and cleanup. See [retry identity](./concepts/operations.md#retry-identity).

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
