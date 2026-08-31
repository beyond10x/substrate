# Design 17: exact resource accounting and hard writable-storage quotas

**Status:** accepted as [ADR 0020](../../adr/0020-writable-storage-uses-delegated-project-quotas.md)
and [ADR 0021](../../adr/0021-execution-metrics-are-explicit-exact-observations.md) ·
**Date:** 2026-08-31

## Problem

An exec already has hard wall-time, cumulative CPU, memory-plus-swap, process-count and output
bounds, but the wire does not report the resources it consumed. A workspace has a per-file ceiling
but no aggregate writable-storage ceiling. Those omissions prevent a caller from both admitting an
ordinary binary under a complete declared envelope and accounting for the resulting run.

This change serves governed reach. It does not add scheduling, billing, product quotas or a second
policy engine.

## Decisions

### Persistent and ephemeral storage are separate authorities

`WorkspaceCreateInput.storage` optionally declares `max_bytes` and `max_inodes`. The quota lasts
with the workspace and is shared by every file API call and every exec mounted at `/workspace`.
`ExecStartInput.scratch` optionally declares the same pair for a fresh private directory mounted at
`/scratch`; that directory is destroyed after the exec. `/tmp` remains tmpfs and is therefore
charged to the exec memory cgroup. System roots, declared host roots and capsules remain read-only.

A byte limit is 1 MiB through 1 TiB and must be divisible by the allocation unit in the capability
snapshot. An inode limit is 16 through 1,048,576. Both values are required together: a byte-only
quota leaves empty-file exhaustion open, and an inode-only quota does not bound data.

The Linux host driver implements the guarantee with project quotas and `PROJINHERIT`. The operator
must delegate an exclusive project-id range with `--project-quota-ids START-END`. Startup creates a
private probe directory and proves byte enforcement, inode enforcement, inheritance, accounting and
cleanup on the configured workspace filesystem. The public fact names the guarantee and allocation
unit, not the filesystem or project id. Without the flag or any part of that proof, the capability
is absent and a quota request is `unserved`; directory-size polling is never substituted.

Quota ids are internal durable allocations. A workspace id is reserved with its project id before
workspace driver dispatch. An exec that asks for scratch is likewise reserved with its project id
before exec dispatch. Recovery retains an allocation until bounded cleanup proves its directory
absent and its accounted usage zero. Exhausting the delegated range is a retriable capacity result.

`EDQUOT` means the declared resource quota stopped a write; `ENOSPC` means the backing filesystem
ran out independently. The file API reports them separately. A process receives the kernel error
and may handle it, so Substrate does not claim quota exhaustion terminated the process without
direct evidence.

### Measurements are requested and exact

`ExecStartInput.measurements` is an optional closed set. Its first and only member is
`resource-usage`. Omission preserves every legacy request and response byte. A measured exec carries
a tagged usage observation: `pending`, `observed`, or `unavailable`.

An observed sample contains monotonic elapsed wall time, cgroup cumulative CPU time, current and
peak memory, current and peak process counts, process-limit hits, OOM-kill count, and cgroup block-I/O
read/write bytes. A scratch request also reports its limit and quota-accounted byte/inode usage.
Current values are absent from a completed sample; cumulative and peak values remain. Values are
captured before cgroup and scratch cleanup and the terminal sample is committed with the terminal
exec observation.

There is deliberately no mean-memory field. Linux exposes an authoritative high-water mark but no
terminal mean; deriving one would make sample cadence and missed ticks part of a value that looks
exact. Block-I/O counters are described as kernel-accounted physical I/O, not logical file bytes.

If a requested counter cannot be read, the driver kills and contains the run and records
`exec.metrics-unavailable`. A restart gap is reported as `unavailable`; wall-clock subtraction is
never used to invent a monotonic duration. Timeout, CPU-budget exhaustion and an observed cgroup OOM
kill receive the distinct existing-observation refusals `exec.timeout`, `exec.cpu-limit` and
`exec.memory-limit`.

### Read and stream surfaces

`GET /v1/metrics?resource_kind=exec&resource_id=…` returns the caller's current or durable terminal
usage. The same route accepts `resource_kind=workspace` and returns its quota limits and current
usage. It never lists another subject's resources.

`WS /v1/metrics/stream?exec_id=…` sends an immediate sample, then samples no faster than once per
second, coalescing pending frames to the newest value. A blocked socket has a bounded write deadline
and is closed without affecting the run. The final frame is emitted only after the terminal sample
is durable. Samples are not retained or replayed; reconnect uses GET.

## Compatibility and contract

Bundle 0.11.0 succeeds 0.10.0, preserves its 31 routes and adds the two metrics routes. Every new
request and resource member is optional and appears only when selected, keeping old conformance
vectors byte-identical. The daemon continues to advertise `substrate-wire/0.4.0`; changing that
header remains a separate consumer migration.

The bundle adds capability predicates, request/result schemas, metrics stream frames, refusal
register entries, positive and negative HTTP vectors, driver vectors for enforcement, and runtime
vectors for live and terminal observations. No released bundle and no hashed renderer is edited.

## Rejected alternatives

- Directory scans can overshoot and race concurrent writers, so they are observations, not quotas.
- `RLIMIT_FSIZE` limits one file and is bypassed by multiple files.
- Replacing `/workspace` with an exec overlay changes persistence semantics.
- Replacing `/tmp` with disk removes the existing memory-accounted scratch surface.
- Retaining metric samples creates a time-series store; Prometheus and retained telemetry remain
  later adapters.
