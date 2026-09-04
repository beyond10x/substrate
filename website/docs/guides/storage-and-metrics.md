---
title: Storage quotas and resource metrics
description: Configure hard writable-storage ceilings and read exact live and terminal execution observations.
---

# Bound writable storage and measure the process that used it

Substrate has two different writable-storage lifetimes:

| Mount | Lifetime | Request field | Intended use |
|---|---|---|---|
| `/workspace` | persists with the workspace | `storage` on workspace create | retained inputs and outputs |
| `/scratch` | exists for one exec | `scratch` on exec start | disposable intermediate data |
| `/tmp` | exists for one exec and is memory-backed | none | small temporary files charged to memory |

Both disk-backed limits combine a byte ceiling and an inode ceiling. A byte-only limit still permits
empty-file exhaustion; an inode-only limit does not bound data.

## Host requirements

Hard disk quotas are served only when the workspace filesystem supports project quotas and the
operator delegates an exclusive project-ID range:

```bash
target/debug/substrate-daemon \
  --socket ./run/substrate.sock \
  --state ./run/state.db \
  --workspaces ./run/workspaces \
  --deployment personal \
  --allow-uid "$(id -u)" \
  --cgroup-root "$CGROUP_ROOT" \
  --project-quota-ids 200000-200511
```

Filesystem quota enablement is an operating-system provisioning step and differs between ext4 and
XFS. Reserve the range for this daemon; sharing it with another quota manager can assign one kernel
project identity to two resources.

At startup, Substrate creates a throwaway project, proves inherited identity plus byte and inode
enforcement, and cleans it up. Only then does `GET /v1/machine` include these facts:

```bash
curl --silent --unix-socket ./run/substrate.sock \
  http://localhost/v1/machine |
  jq '.result.facts | {
    workspace_quota: .["workspace.storage-quota"],
    scratch_quota: .["exec.scratch-quota"],
    resource_usage: .["exec.resource-usage"],
    metrics_stream: .["metrics.stream"]
  }'
```

If a fact is absent, a request that needs it is refused by name. Substrate never substitutes a
directory-size scan for a hard quota.

## Request a persistent workspace ceiling

`max_bytes` is aligned to the advertised `allocation_unit_bytes` value:

```json
{
  "op": "01JQUOTAWORKSPACE00000001",
  "input": {
    "source": "empty",
    "labels": {"job": "bounded-build"},
    "storage": {
      "max_bytes": 268435456,
      "max_inodes": 8192
    }
  }
}
```

The workspace observation reports its limit, quota-accounted bytes and inodes, and observation
time. `GET /v1/metrics?resource_kind=workspace&resource_id=ws_…` refreshes that observation before
returning it.

## Request per-exec scratch and counters

An exec opts into counters explicitly and can independently ask for `/scratch`:

```json
{
  "scratch": {
    "max_bytes": 33554432,
    "max_inodes": 1024
  },
  "measurements": ["resource-usage"]
}
```

The full request also carries the workspace, argv, environment, sandbox snapshot, process limits,
and wait preference. See [run a bounded command](./run-a-command.md) for a complete terminal flow.

## Read the latest observation

```bash
curl --silent --unix-socket ./run/substrate.sock \
  "http://localhost/v1/metrics?resource_kind=exec&resource_id=$EXEC" | jq .result
```

A running observation can include current memory and process counts. A terminal observation sets
`complete: true` and retains exact cumulative or peak facts:

```json
{
  "resource_kind": "exec",
  "exec": "ex_…",
  "usage": {
    "status": "observed",
    "complete": true,
    "wall_time_us": 812734,
    "cpu_time_us": 274091,
    "memory_peak_bytes": 18874368,
    "processes_peak": 2,
    "process_limit_hits": 0,
    "memory_oom_kills": 0,
    "io_read_bytes": 4096,
    "io_write_bytes": 16777216,
    "scratch": {
      "limit": {"max_bytes": 33554432, "max_inodes": 1024},
      "used_bytes": 16777216,
      "used_inodes": 2,
      "observed_at": "2026-08-31T12:00:00Z"
    }
  }
}
```

The numbers above illustrate the response shape; actual values come from the kernel. Counters that
do not have a meaningful terminal “current” value are absent, not replaced by zero.

## Stream without building a history service

`GET /v1/metrics/stream?exec_id=ex_…` upgrades to WebSocket and emits one immediate observation,
then the latest sample approximately once per second. Slow readers skip intermediate samples. The
stream has no replay history and closes after a terminal or unavailable observation.

The stream is bounded, and the bounds a client can act on carry a code. A subject may hold 4 metrics
streams at once and a deployment 64; one more is refused `429` with the code
`metrics.stream-capacity`, an `exhausted` refusal worth retrying once a stream you hold has ended. A
client may send 120 control frames a minute, and the one after that closes the stream `1008`. The
stream carries samples one way and accepts control frames only, so a data frame closes it `1003`.
Two bounds end the connection instead of naming themselves: a stream is cut after 1 hour, and a
client frame larger than 1024 bytes is refused by the socket before the daemon sees it. Reconnect in
each case; nothing is lost, because the stream carries no history to lose.

Use the durable terminal exec observation when you need a record. Use the WebSocket when you need a
live display or a local admission signal. Substrate does not turn sampled telemetry into billing or
long-term monitoring data.
