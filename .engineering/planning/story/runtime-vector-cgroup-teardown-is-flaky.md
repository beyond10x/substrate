---
format: aep.planning-md/3
id: story:runtime-vector-cgroup-teardown-is-flaky
kind: story
status: implemented
title: The runtime-vector delegated lane tears its cgroups down deterministically
scope:
- confidence: cited
  path: crates/substrate-daemon/tests/runtime_vectors.rs
revision: 8
transitions:
- {from: "draft", to: "proposed", at: "2026-10-14T17:24:21Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"review_outcome":5}}}
- {from: "proposed", to: "active", at: "2026-10-14T17:24:21Z", actor: "human:timo", revision: 7, decided_on: {"recorded":{"review_outcome":5}}}
- {from: "active", to: "implemented", at: "2026-10-14T22:37:45Z", actor: "human:timo", revision: 8, decided_on: {"recorded":{"test_result":1,"review_outcome":5}}}
---
# Story: The runtime-vector delegated lane tears its cgroups down deterministically

## Context

`crates/substrate-daemon/tests/runtime_vectors.rs` fails intermittently on its delegated lane with

```
runtime_vectors.rs:78: per-test daemon cgroup … was not empty at teardown: Device or resource busy
```

Measured during the 2026-09-04 security wave, on `617bbed` itself: **1 red of 2** on the base
commit, and 2 red of 4 on a branch whose diff touches neither `/v1/execs` nor cgroup teardown
(`review-result:adversary-u3-pass-1`). One red additionally showed `runtime_vectors.rs:1224`
returning `500 operation.outcome-unknown` on exec 53 of 129.

The lane is the only proof that the shipped binary answers the wire as its vectors say. A lane that
fails one run in two cannot be read as evidence either way.

## Acceptance

One of two outcomes, both observable:

1. **Harness race.** `bash scripts/delegated-lane.sh` run ten times consecutively on an unchanged
   tree exits 0 ten times, with the fix confined to the test harness and adding no wait for a process or child group the
   daemon left behind (that is outcome 2).
2. **The daemon is the cause.** If a traced run shows a process or a child cgroup still in the
   per-test group after the daemon reported its exec terminal, or the `500 operation.outcome-unknown`
   symptom traces to the daemon rather than the harness, a new draft story records that trace (the pid or child group, its
   cgroup path and the terminal observation it outlived, or the failing request and its daemon log), this story's report links it, and the
   harness gains no wait, retry or relaxed assertion that would hide it. This story then closes as
   superseded by that draft.

## Notes

`Device or resource busy` on an rmdir of a cgroup directory means a process is still in it, or a
child group still exists. The teardown is racing something it does not wait for — most likely the
exec's own children, or the daemon's cleanup of the per-test group. The investigation reads
`cgroup.procs` and `cgroup.events` of the group at the failing rmdir before choosing an outcome.

## Parallel work

Lands on `crates/substrate-daemon/tests/runtime_vectors.rs`, and on `scripts/delegated-lane.sh` only
if the investigation places the race there. No other unit of this wave touches either. Under outcome
2 the daemon half is filed, not fixed, because `sessions.rs` and `runtime.rs` belong to the
session-containment unit in this wave. A harness-only change owes no `CHANGELOG.md` entry.
