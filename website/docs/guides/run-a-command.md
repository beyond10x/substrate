---
title: Run a bounded command
description: A copy-and-paste terminal walkthrough for executing an ordinary binary with confinement, limits, output, and metrics.
---

# Execute an ordinary binary inside a measured resource envelope

This walkthrough uses `curl` and `jq`; no agent runtime is involved. It creates a workspace, writes
an input file, runs `sha256sum` directly, polls exact resource observations, and reads bounded
output.

The daemon must first prove the complete Linux confinement floor. For resource metrics it must also
publish `exec.resource-usage`; for hard `/workspace` and `/scratch` ceilings it must publish the two
quota facts described in [storage quotas and resource metrics](./storage-and-metrics.md).

## 1. Point the shell at the daemon

```bash
SOCKET=./run/substrate.sock
BASE=http://localhost

MACHINE=$(curl --silent --show-error --unix-socket "$SOCKET" "$BASE/v1/machine")
SNAPSHOT=$(jq -r '.result.snapshot' <<<"$MACHINE")

jq '.result.facts | {
  namespaces: .["exec.namespaces"],
  limits: .["exec.cgroup-limits"],
  no_egress: .["exec.no-egress"],
  metrics: .["exec.resource-usage"],
  workspace_quota: .["workspace.storage-quota"],
  scratch_quota: .["exec.scratch-quota"]
}' <<<"$MACHINE"
```

Do not manufacture a snapshot value or assume that a missing fact is supported. The snapshot binds
the request to the exact capability generation that was inspected.

## 2. Create a disk-bounded workspace

This request caps persistent data at 64 MiB and 2,048 inodes:

```bash
CREATE_BODY=$(jq -nc '{
  op: "01JDEMO_CREATE_WORKSPACE_01",
  input: {
    source: "empty",
    labels: {purpose: "terminal-demo"},
    storage: {max_bytes: 67108864, max_inodes: 2048}
  }
}')

CREATE=$(curl --silent --show-error --unix-socket "$SOCKET" \
  --header 'content-type: application/json' \
  --data "$CREATE_BODY" \
  "$BASE/v1/workspaces")

WS=$(jq -r '.result.id' <<<"$CREATE")
printf 'workspace: %s\n' "$WS"
```

If the host did not prove project quotas, this returns
`workspace.storage-quota-unserved` and creates nothing. To explore guarded workspace I/O without a
disk ceiling, omit `storage`; that is a different and deliberately weaker request.

## 3. Put input in `/workspace`

The file API takes standard base64. The process later sees this file at `/workspace/input.txt`:

```bash
CONTENT=$(printf '%s' 'substrate runs ordinary binaries' | base64 -w0)
WRITE_BODY=$(jq -nc --arg content "$CONTENT" '{
  op: "01JDEMO_WRITE_INPUT_000001",
  input: {content: {encoding: "base64", data: $content}}
}')

curl --silent --show-error --unix-socket "$SOCKET" \
  --request PUT \
  --header 'content-type: application/json' \
  --data "$WRITE_BODY" \
  "$BASE/v1/workspaces/$WS/files/input.txt" | jq .result
```

## 4. Start the binary

The request below gives the process:

- 15 seconds of wall time;
- 2 seconds of cumulative CPU time;
- 64 MiB of memory plus swap bound by the host;
- at most 16 processes;
- at most 64 KiB of retained stdout and stderr;
- a 32 MiB, 1,024-inode `/scratch` directory;
- no network interface except loopback inside its namespace;
- explicit exact resource accounting.

```bash
EXEC_BODY=$(jq -nc --arg ws "$WS" --arg snapshot "$SNAPSHOT" '{
  op: "01JDEMO_EXEC_SHA256SUM_001",
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

START=$(curl --silent --show-error --unix-socket "$SOCKET" \
  --header 'content-type: application/json' \
  --data "$EXEC_BODY" \
  "$BASE/v1/execs")

EXEC=$(jq -r '.result.id' <<<"$START")
printf 'exec: %s\n' "$EXEC"
```

Substrate executes the argument vector as given. It does not concatenate arguments into a shell
command. To run a shell pipeline, make the shell explicit—such as
`["/usr/bin/sh", "-c", "sort /workspace/input.txt"]`—and treat the command string as shell input in
your own security model. Direct argv is easier to reason about when values come from another user.

## 5. Watch current usage

Poll the latest exact observation while the command is running:

```bash
while :; do
  SAMPLE=$(curl --silent --show-error --unix-socket "$SOCKET" \
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

For a UI, the WebSocket route `/v1/metrics/stream?exec_id=$EXEC` sends an immediate latest sample
and then samples at the advertised interval. It is latest-wins rather than a replay log.

## 6. Read the result and bounded output

```bash
curl --silent --show-error --unix-socket "$SOCKET" \
  "$BASE/v1/execs/$EXEC" | jq '.result | {state, exit, refusal, usage}'

curl --silent --show-error --unix-socket "$SOCKET" \
  "$BASE/v1/execs/$EXEC/output?stream=stdout&offset=0&limit_bytes=65536" |
  jq -r '.result.content.data' | base64 -d
```

A non-zero child exit is still a successful observation of a process that ran. A resource ceiling
can leave the exec in a terminal state with a named `refusal`, such as `exec.cpu-limit` or
`exec.memory-limit`. An unavailable confinement guarantee instead prevents dispatch and returns an
HTTP refusal.

## 7. Clean up

```bash
curl --silent --show-error --unix-socket "$SOCKET" \
  --request DELETE \
  --header 'content-type: application/json' \
  --data '{"op":"01JDEMO_DESTROY_WORKSPACE_1","input":{}}' \
  "$BASE/v1/workspaces/$WS" | jq .result
```

`/scratch` is removed as part of terminal exec cleanup. The workspace remains until destroyed or
expired by its lease.
