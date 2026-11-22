---
title: Run a bounded command
description: A copy-and-paste terminal walkthrough for executing an ordinary binary with confinement, limits, output, and metrics.
---

# Execute an ordinary binary inside a measured resource envelope

This is the W1 → O1 → X1 journey from [operations and observations](../concepts/operations.md),
using server-issued resource IDs and caller-issued operation IDs.

This walkthrough uses `curl` and `jq` and no agent runtime. It creates a workspace, writes
an input file, runs `sha256sum` directly, polls exact resource observations, and reads bounded
output.

## Start with an observable machine

The safest first run uses the personal deployment over a Unix socket. With it you inspect machine
facts and run workspace operations, and it makes no claim that this host can execute a process
safely. It needs:

- Linux
- Rust 1.97 or newer
- `curl`
- `jq` for readable JSON output

Process execution also requires Bubblewrap, `/usr/bin/socat`, and a delegated cgroup v2 subtree.
They are not needed to start the daemon and inspect its facts. The probe uses `socat` to prove that
a sandbox cannot reach the host's Unix socket namespace.

### Build the workspace

From a Substrate checkout:

```bash
cargo build --workspace --locked
```

### Run the daemon

Create a private runtime directory and start the daemon with your numeric user ID allowed:

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

The daemon derives the subject from kernel peer credentials on each Unix-socket connection. No
request header or JSON field can choose a subject.

### Inspect verified facts

In another terminal:

```bash
curl --silent --show-error --fail-with-body --max-time 30 \
  --unix-socket ./run/substrate.sock \
  http://localhost/v1/machine | jq
```

Read the capability document literally. If execution confinement is unavailable, its facts remain
absent. A later exec request is refused with `exec.sandbox-unavailable`, and Substrate does not fall
back to an ordinary host process.

### What to look for

The response describes the deployment, contract identity, driver generation, verified capabilities,
and operational limits. Each is an observation of this running daemon, and none is inferred from
its configuration.

### Try a workspace without process execution

This first round trip creates workspace W1, writes the input used by the numbered steps, reads it
back, and destroys the workspace. It needs guarded workspace I/O and requests neither an exec nor a
hard disk quota. Run it as a Bash script from the checkout, with the daemon still running.

The helper fails on transport, HTTP and response-envelope errors. A fresh run gets fresh operation
IDs. If a connection fails, keep the printed run ID and request bodies to reconcile the same
operations instead of repeating effects.

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
for inspection and cleanup. See [retry identity](../concepts/operations.md#retry-identity).

### Serving process execution

A Linux host that serves exec needs all of the following:

1. bubblewrap available at the configured path;
2. user, mount, PID, IPC, UTS, and network namespaces;
3. a delegated cgroup v2 subtree with `cpu`, `memory`, and `pids` controllers;
4. a process-free delegation root;
5. `/usr/bin/socat` for the host-IPC confinement probe;
6. the delegation passed with `--cgroup-root`.

An exec's CPU bandwidth follows its declared CPU time over its timeout, capped at
`--exec-cpu-cores N` whole cores (default 1). The daemon refuses to start with 0 or with more cores
than it may run on.

The daemon probes the backend before advertising execution. Missing enforcement produces a refusal,
not weaker isolation. Read [confinement and refusal](../concepts/confinement.md) before admitting
process work, or [deployment postures](./deployment.md) before making the daemon reachable beyond
its owner.

The daemon must first prove the complete Linux confinement floor. For resource metrics it must also
publish `exec.resource-usage`. For hard `/workspace` and `/scratch` ceilings it must publish the two
quota facts described in [storage quotas and resource metrics](./storage-and-metrics.md).

Run the Bash blocks of the numbered steps in order in one script. `set -euo pipefail` stops on an
error. If a response is lost, the printed run ID and the request bodies identify the operations to
reconcile. Starting the whole script again uses new IDs and can create new effects.

## 1. Point the shell at the daemon

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

MACHINE=$(api "$BASE/v1/machine")
SNAPSHOT=$(jq -er '.result.snapshot | select(type == "string" and length > 0)' <<<"$MACHINE")

jq '.result.facts | {
  namespaces: .["exec.namespaces"],
  limits: .["exec.cgroup-limits"],
  no_egress: .["exec.no-egress"],
  metrics: .["exec.resource-usage"],
  workspace_quota: .["workspace.storage-quota"],
  scratch_quota: .["exec.scratch-quota"]
}' <<<"$MACHINE"

jq -e '.result.facts |
  .["exec.namespaces"] != null and .["exec.cgroup-limits"] != null and
  .["exec.no-egress"] == true and .["exec.resource-usage"] != null and
  .["workspace.storage-quota"] != null and .["exec.scratch-quota"] != null' \
  >/dev/null <<<"$MACHINE" || {
    printf '%s\n' 'This daemon does not advertise every fact required by this guide.' >&2
    exit 1
  }
```

Do not manufacture a snapshot value or assume that a missing fact is supported. The snapshot binds
the request to the exact capability generation that was inspected.

## 2. Create a disk-bounded workspace

The preceding fact check stops before creating anything if this host cannot serve the full example.
To try file I/O alone, use [the workspace round trip](#try-a-workspace-without-process-execution).

This request caps persistent data at 64 MiB and 2,048 inodes:

```bash
CREATE_BODY=$(jq -nc --arg op "$RUN_ID-create" '{
  op: $op,
  input: {
    source: "empty",
    labels: {purpose: "terminal-demo"},
    storage: {max_bytes: 67108864, max_inodes: 2048}
  }
}')

CREATE=$(api \
  --header 'content-type: application/json' \
  --data "$CREATE_BODY" \
  "$BASE/v1/workspaces")

WS=$(jq -er '.result.id | select(type == "string" and length > 0)' <<<"$CREATE")
printf 'workspace: %s\n' "$WS"
```

If the host did not prove project quotas, this request returns
`workspace.storage-quota-unserved` and creates nothing. To explore guarded workspace I/O without a
disk ceiling, omit `storage`, which makes a different and weaker request.

## 3. Put input in `/workspace`

The file API takes standard base64. The process later sees this file at `/workspace/input.txt`:

```bash
CONTENT=$(printf '%s' 'substrate runs ordinary binaries' | base64 -w0)
WRITE_BODY=$(jq -nc --arg op "$RUN_ID-write" --arg content "$CONTENT" '{
  op: $op,
  input: {content: {encoding: "base64", data: $content}}
}')

api \
  --request PUT \
  --header 'content-type: application/json' \
  --data "$WRITE_BODY" \
  "$BASE/v1/workspaces/$WS/files/input.txt" | jq .result
```

## 4. Start the binary

The request gives the process:

- 15 seconds of wall time;
- 2 seconds of cumulative CPU time;
- 64 MiB of memory plus swap bound by the host;
- at most 16 processes;
- at most 64 KiB of retained stdout and stderr;
- a 32 MiB, 1,024-inode `/scratch` directory;
- no network interface except loopback inside its namespace;
- explicit exact resource accounting.

```bash
EXEC_BODY=$(jq -nc --arg op "$RUN_ID-exec" --arg ws "$WS" --arg snapshot "$SNAPSHOT" '{
  op: $op,
  input: {
    workspace: $ws,
    argv: ["/usr/bin/sha256sum", "/workspace/input.txt"],
    env: {allow: ["PATH", "LANG"], set: {}},
    sandbox: {
      capability_snapshot: $snapshot,
      network: "none",
      profile: "workspace",
      require: true
    },
    limits: {
      timeout_ms: 15000,
      output_bytes: 65536,
      processes: 16,
      memory_bytes: 67108864,
      cpu_millis: 2000
    },
    scratch: {max_bytes: 33554432, max_inodes: 1024},
    measurements: ["resource-usage"],
    wait: false
  }
}')

START=$(api \
  --header 'content-type: application/json' \
  --data "$EXEC_BODY" \
  "$BASE/v1/execs")

EXEC=$(jq -er '.result.id | select(type == "string" and length > 0)' <<<"$START")
printf 'exec: %s\n' "$EXEC"
```

Substrate executes the argument vector as given and does not concatenate arguments into a shell
command. A shell pipeline needs an explicit shell, such as
`["/usr/bin/sh", "-c", "sort /workspace/input.txt"]`. Treat the command string as shell input in
your own security model. Direct argv is easier to reason about when values come from another user.

## 5. Watch current usage

Poll the latest exact observation while the command is running:

```bash
for attempt in {1..20}; do
  SAMPLE=$(api \
    "$BASE/v1/metrics?resource_kind=exec&resource_id=$EXEC")
  jq '.result.usage | {
    status, complete, wall_time_us, cpu_time_us,
    memory_current_bytes, memory_peak_bytes,
    processes_current, processes_peak,
    io_read_bytes, io_write_bytes
  }' <<<"$SAMPLE"
  jq -e '.result.usage.complete == true or .result.usage.status == "unavailable"' \
    >/dev/null <<<"$SAMPLE" && break
  sleep 1
done
```

Polling stops after at most 20 samples. That limit is a client observation bound and does not prove
that the exec finished. The next step reads process state. If a request times out, reconcile its
existing ID.

For a UI, the WebSocket route `/v1/metrics/stream?exec_id=$EXEC` sends an immediate latest sample
and then samples at the advertised interval. The stream is latest-wins and is not a replay log.

## 6. Read the result and bounded output

```bash
OBSERVED=$(api "$BASE/v1/execs/$EXEC")
jq '.result | {state, exit, refusal, usage}' <<<"$OBSERVED"
jq -e '.result.state | . == "exited" or . == "cancelled" or . == "expired"' \
  >/dev/null <<<"$OBSERVED" || {
    printf '%s\n' 'Exec is not proven terminal; reconcile before cleanup or another run.' >&2
    exit 1
  }

api \
  "$BASE/v1/execs/$EXEC/output?stream=stdout&offset=0&limit_bytes=65536" |
  jq -er '.result.content.data | select(type == "string")' | base64 -d
```

A non-zero child exit is still a successful observation of a process that ran. A resource ceiling
can leave the exec in a terminal state with a named `refusal`, such as `exec.cpu-limit` or
`exec.memory-limit`. An unavailable confinement guarantee instead prevents dispatch and returns an
HTTP refusal.

## 7. Clean up

```bash
RETIRE_BODY=$(jq -nc --arg op "$RUN_ID-retire" '{op: $op, input: {}}')
api --request DELETE --header 'content-type: application/json' \
  --data "$RETIRE_BODY" "$BASE/v1/execs/$EXEC" | jq .result

DESTROY_BODY=$(jq -nc --arg op "$RUN_ID-destroy" '{op: $op, input: {}}')
api --request DELETE --header 'content-type: application/json' \
  --data "$DESTROY_BODY" "$BASE/v1/workspaces/$WS" | jq .result
```

`/scratch` is removed as part of terminal exec cleanup. Exec retirement releases its retained resource
record. The workspace remains until destroyed or expired by its lease.
