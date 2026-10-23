---
format: aep.planning-md/3
id: review-result:wave-2026-10-23-u2-adversary-pass-1
kind: review-result
status: active
title: Adversary pass 1, unit u2 (unknown exec killed again)
relations:
- reviews: story:an-unknown-exec-is-killed-again-before-restart
revision: 1
---
# Adversary pass 1, unit u2 (unknown exec killed again)

Agent type `aep:adversary`. Branch `unit/2026-10-23-u2-unknown-exec`, working tree on `344f9c56`. Cases in `crates/substrate-daemon/tests/pipe_session.rs` (module `adversary_unknown_exec_retry`). Verdict: NEEDS-CHANGE.

```findings
- file: crates/substrate-store/src/leases.rs
  line: 559
  category: acceptance
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: undecided
  message: lease cleanup rewrites an exec recorded unknown to expired even when its tree was never proven empty, so the daemon reports it terminated (pipe_session an_unknown_exec_whose_lease_expires_is_still_reported_unknown is red)
- file: crates/substrate-host/src/process.rs
  line: 2830
  category: mutant
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: no unit case drives run_child into an unproven cgroup, so deleting the cgroup_unproven store leaves the unit's suite green while disabling the whole retry
- file: crates/substrate-host/src/process.rs
  line: 3326
  category: acceptance
  severity: warning
  verdict: INFEASIBLE
  origin: pre-existing
  message: a dispatch-failure OutcomeUnknown with an unproven cgroup is not tracked, so nothing retries its tree before restart
- file: crates/substrate-daemon/tests/pipe_session.rs
  line: 3400
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: two existing test doc comments still say nothing retries an unknown exec's kill before restart, which this unit made false
```

Coordinator decisions: 1 fixed in correction round 1 (a stored `unknown` stays `unknown` on lease expiry; only the lease becomes expired); 2 covered by the adversary's delegated case `a_real_exec_left_unproven_by_run_child_is_killed_by_the_next_round` and the security reviewer's run_child case; 3 merged with security finding 5 and routed to correction round 1; 4 fixed in correction round 1.
