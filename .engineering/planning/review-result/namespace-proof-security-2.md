---
format: aep.planning-md/3
id: review-result:namespace-proof-security-2
kind: review-result
status: active
title: Security review of the namespace driver's proof holding, pass 2
relations:
- reviews: story:namespace-driver-start-holds-its-proof-until-pod-create
revision: 1
---
# Security review of the namespace driver's proof holding, second pass

Reviewed: branch `story/namespace-proof` at `64048e0ce`, by a second independent reviewer on
2026-11-23. Verdict: the comparison is after the last wait and holds every arm § 6.3 claims.
Four findings: three closed on the branch, one exists on `main` and is unchanged.

## What was run

- Kubernetes library: 201 cases with the reviewer's six, two of them red.
- Daemon `namespace_admission`: 9 passed.
- After the fixes: 203 cases passed on the second run; the first run had one failure in an
  existing timing case at a load average of 131, which the reviewer's fourth finding names.
- Clippy on both crates and `cargo fmt --all --check`: exit 0.

## What was read and found correct

- Every wait between the daemon's admission and the create: the workspace stripe lock, the
  workspace admission, the store reservation, the ledger read, two placement reads and the
  paged policy list. The comparison is after all of them; only a synchronous render follows.
- The remaining window is the create's own connect, TLS and send, bounded at 30 s, on one
  connection without a retry.
- Only one function writes the memory of stated snapshots, from the same value it returns. A
  caller cannot grow it, and eviction can only make a start stale. No lock is held across an
  await.
- Both refusals are recorded as refused, with the exec row deleted and unbound backend rows
  settled.

## Not checked

Mutants by real edits and Clippy on the reviewer's own file (the build pool's disk floor and
the machine load), the delegated and namespace lanes, and any race between the binding re-read
and the snapshot.

```findings
[
{"file":"crates/substrate-kubernetes/src/namespace/probe.rs","line":525,"category":"contract-drift","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"Validity ran on the monotonic clock while valid_until is rendered from the wall clock, so after a forward clock step or a suspend the snapshot stated facts beside a passed valid_until, which design 31's machine group refuses. Closed: a proof ends when either clock says so; the wall clock can end a proof early and never keeps one valid."},
{"file":"crates/substrate-kubernetes/src/exec.rs","line":329,"category":"integrity","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"The lookup took the newest remembered entry for a digest, so a start admitted under one Lease would be served under the next when both snapshots share a digest; only two probes at the same wall-clock nanosecond produce that. Closed: a digest remembered with two different statements is exec.capability-stale."},
{"file":"docs/design/34-kubernetes-namespace-driver.md","line":294,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"The stated reason for 2,048 named the 60 s minimum as the probe interval and bounded one run at 24 h, while a start can queue behind several runs on the workspace lock. Closed: the design states the memory as 2,048 probes (34 h at 60 s, 14 days at the default 600 s) and that a longer wait is stale."},
{"file":"crates/substrate-kubernetes/src/exec_tests.rs","line":2133,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"pre-existing","message":"This case and driver_tests.rs:1125 race a sleep or poll against a delayed policy read inside one join and fail when the test thread stalls 300 to 500 ms. Unchanged on this branch; the same constants are on main."}
]
```
