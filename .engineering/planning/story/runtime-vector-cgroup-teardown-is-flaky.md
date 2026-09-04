---
format: aep.planning-md/1
id: story:runtime-vector-cgroup-teardown-is-flaky
kind: story
status: draft
title: The runtime-vector delegated lane tears its cgroups down deterministically
scope:
- confidence: cited
  path: crates/substrate-daemon/tests/runtime_vectors.rs
revision: 2
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
fails one run in two cannot be read as evidence either way, and it teaches every agent and every CI
consumer to re-run rather than to look.

## Acceptance

`bash scripts/delegated-lane.sh` run ten times consecutively on an unchanged tree exits 0 ten times.

## Notes

`Device or resource busy` on an rmdir of a cgroup directory means a process is still in it, or a
controller is still attached. The teardown is racing something it does not wait for — most likely
the exec's own children, or the daemon's cleanup of the per-test group.

This is a test-harness defect, not a daemon defect, unless the investigation shows the daemon leaves
the group populated after it reports the exec terminal — in which case it is the more interesting
finding and this story should say so.
