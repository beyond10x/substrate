---
format: aep.planning-md/3
id: review-result:wave-2026-10-23-u2-adversary-pass-2
kind: review-result
status: active
title: Adversary pass 2, unit u2 (unknown exec killed again)
relations:
- reviews: story:an-unknown-exec-is-killed-again-before-restart
revision: 1
---
# Adversary pass 2, unit u2 (unknown exec killed again)

Agent type `aep:adversary`. Branch `unit/2026-10-23-u2-unknown-exec`, working tree on `344f9c56` after correction round 1. Cases in `crates/substrate-store/src/tests.rs` (module `adversary_2_unknown_exec_lease_expiry`) and `crates/substrate-host/tests/unproven_exec_tree_retry_adversary_2.rs`. Verdict: NEEDS-CHANGE, 2 red.

```findings
- file: crates/substrate-store/src/leases.rs
  line: 562
  category: acceptance
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: keeping an expired-lease exec unknown makes it unretirable (exec.not-terminal) forever, so the row and its exec-capacity slot can never be reclaimed, where the base made it expired and retirable
- file: crates/substrate-host/src/process.rs
  line: 1802
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: undecided
  message: signal answers NotFound for a carried unproven tree while observe answers Failed, and workspace-lease cleanup (service.rs:832) reads that NotFound as absent and destroys the workspace and releases its quota under a tree not proven gone
```

Coordinator decisions: F2 fixed by making signal answer exec.cgroup-not-empty for a carried id, as observe does; F1 fixed by letting retire admit an unknown exec once the recovery pass has recorded it physically absent, and refusing it before that.
