---
format: aep.planning-md/3
id: review-result:wave-2026-10-23-u2-security-pass-1
kind: review-result
status: active
title: Security review pass 1, unit u2 (unknown exec killed again)
relations:
- reviews: story:an-unknown-exec-is-killed-again-before-restart
revision: 1
---
# Security review pass 1, unit u2 (unknown exec killed again)

Agent type `aep:security-reviewer`. Branch `unit/2026-10-23-u2-unknown-exec`, working tree on `344f9c56`. Cases in `crates/substrate-host/tests/unproven_exec_tree_retry_security.rs` (2 delegated, green). Verdict: broken at the daemon level (F1); host-side kill confinement holds.

```findings
- file: crates/substrate-store/src/leases.rs
  line: 555
  category: acceptance
  severity: blocker
  verdict: CONFIRMED
  origin: pre-existing
  message: lease expiry rewrites an exec recorded unknown to the terminal state expired whether or not its tree was proven gone, so acceptance clause 2 fails (pipe_session.rs adversary_unknown_exec_retry::an_unknown_exec_whose_lease_expires_is_still_reported_unknown is red)
- file: crates/substrate-host/src/process.rs
  line: 1772
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: each unproven exec reached through signal costs up to 100 ms one after another, measured at about 100 ms per tracked tree during shutdown; bounded, but about 13 s shutdown at 128 tracked
- file: crates/substrate-host/src/process.rs
  line: 3742
  category: integrity
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: any stat failure on cgroup.procs, EACCES included, is read as NotFound, so kill_unproven forgets the entry and frees its slot without proof
- file: crates/substrate-host/src/process.rs
  line: 1651
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: pre-existing
  message: nested cgroups are killed but never removed depth-first, so such an entry holds its slot until a restart that refuses; a confined child cannot create one
- file: crates/substrate-host/src/process.rs
  line: 3315
  category: judgement
  severity: warning
  verdict: NEEDS-CHANGE
  origin: undecided
  message: a post-spawn containment that ends OutcomeUnknown leaves an unproven cgroup that nothing tracks or retries before restart
```

Coordinator decisions: F1 fixed in correction round 1 (same as adversary F1); F2 accepted as bounded; F3 fixed in correction round 1 (only ENOENT counts as gone); F4 accepted, pre-existing and unreachable from a confined child; F5 fixed in correction round 1 if it is a registration in the existing unproven map, otherwise left open and reported.
