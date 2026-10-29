---
format: aep.planning-md/3
id: story:test-daemon-readiness-under-load
kind: story
status: draft
title: A test daemon's readiness deadline holds on a loaded machine
relations:
- decomposes: epic:host-run-gates
revision: 1
---
# Story: a test daemon's readiness deadline holds on a loaded machine

## Observation

The local full gate on `wave/adapter-io` failed twice on daemon readiness deadlines, in suites the
wave did not touch, on 2026-10-29:

- run at `7bf25495`: `crates/substrate-daemon/tests/hosted_pool_adversary_pass1.rs:370`, "TLS daemon
  readiness timed out", in 3 of 9 cases; the same binary run alone passed 9 of 9 in 37.79 s.
- run at `2e8337fb`: `crates/b10x-substrate-cli/tests/adversary_pass2.rs:76`, "managed daemon starts:
  Startup(\"readiness deadline elapsed (substrate transport failed: No such file or directory (os
  error 2)): \")", in 10 cases.

`uptime` right after them read load averages of 79 (5-minute) and 133 (1-minute). The machine was
shared with other sessions' builds. The coordinating session accepted the pull request's CI gate as
the gate of record for that wave.

## Acceptance

- Every test helper that waits for a daemon to become ready takes its deadline from one shared
  place, and the deadline is long enough that the two suites above pass under a stated synthetic
  load (for example 4 busy loops per core) on a workstation; the run is recorded in the store, not
  repeated in CI.
- A daemon that never becomes ready still fails the case, with the daemon's own stderr in the
  message.
- No case gains a retry loop, and CI time grows by at most 1 minute.

## Not known

Whether the time goes to the daemon's own startup probes (bubblewrap, cgroup, capability snapshot)
or to scheduling; the first step measures it.
