---
format: aep.planning-md/3
id: review-result:wave-2026-11-01-d354-u3-adversary-pass-1
kind: review-result
status: active
title: D354 U3 adversary pass 1
relations:
- reviews: story:listener-held-during-install-is-never-inherited-by-a-fork
revision: 1
---
# Adversary pass 1 — wave D354 U3 (story:listener-held-during-install-is-never-inherited-by-a-fork)

Diff `91c8163a...5b6ae087`; adversary commit `2ca82d24` on `unit/d354-u3`, merged into the integration branch at `d1a60105`. Agent `aep:adversary`. Header and findings verbatim; prose trimmed to the measured facts.

```
verdict: green
cases: executed 404→408, red 1
origin: introduced 0, pre-existing 1, undecided 0
```

Commands, each after the wave pre-check: host --lib egress 36/0 on the committed tree; clippy -D warnings clean; host --lib with the red case present 407 passed, 1 failed (the red case). Peak target/ 904M, lowest / 82G.

Cases committed (`crates/substrate-host/src/egress.rs`, `mod d354_adv`):

- `no_process_but_its_forwarder_holds_a_listener_once_install_returns`: three apertures, a neighbour thread forking before, during and 200 ms after each report, children alive 3 s; after `install_all` returns no process but the forwarders holds a listener inode, and each forwarder holds exactly one descriptor above 2.
- `a_forwarder_that_dies_before_its_report_is_refused_at_once_and_reaped`: refused as stage 11 within 500 ms, forwarder reaped.
- `a_refused_stage_leaves_the_installing_thread_no_child`: stage 2 (cgroup, errno 28 via `/dev/full`) and stage 6 (bind) leave no child and no zombie.

Red case outside the Acceptance, not committed: `a_forwarder_that_dies_before_its_report_is_refused_at_once_while_another_thread_forks` (a 20-round local loop). In 2 of 20 rounds the refusal took 2.0096 s and 2.0094 s after the kill: a kept-alive fork between `handback_pair()` (:832) and `drop(child_end)` (:868) holds the report socket's other end, so `receive_report` (:869) sees EOF only when that child exits. Only kept-alive test forks reach it; the old `listen_inside` hand-back had the same window. Kept as local evidence (a loop cannot enter the suite, AGENTS.md § The gate), saved as a patch outside the repository.

```findings
[{"file": "crates/substrate-host/src/egress.rs", "line": 4113, "category": "acceptance", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "scope in; case no_process_but_its_forwarder_holds_a_listener_once_install_returns: after install_all returns, with a neighbour's children forked before, during and after each report still alive, no process but the forwarders holds a listener inode, and each forwarder holds exactly one descriptor above 2"},
 {"file": "crates/substrate-host/src/egress.rs", "line": 1944, "category": "judgement", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "scope in; case none: reach_expecting is test-only and cannot copy a listener because the installing process never holds one; production forks exec with CLOEXEC, run keep_only first, or exit at once"},
 {"file": "crates/substrate-host/src/egress.rs", "line": 3948, "category": "acceptance", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "scope in; case a_forwarder_that_dies_before_its_report_is_refused_at_once_and_reaped: a forwarder killed before its report is refused as stage 11 within 500 ms and reaped while its helper is still retrying a bind"},
 {"file": "crates/substrate-host/src/egress.rs", "line": 3976, "category": "boundary", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "scope in; case a_refused_stage_leaves_the_installing_thread_no_child: refusals at stage 2 (cgroup, errno 28) and stage 6 (bind) leave the installing thread with no child and no zombie"},
 {"file": "crates/substrate-host/src/egress.rs", "line": 869, "category": "concurrency", "severity": "note", "verdict": "CONFIRMED", "origin": "pre-existing", "message": "scope out; case a_forwarder_that_dies_before_its_report_is_refused_at_once_while_another_thread_forks (red, uncommitted, a 20-round local loop): a kept-alive fork landing between handback_pair (:832) and drop(child_end) (:868) holds the report socket's other end, so a forwarder dead before its report was refused only after 2.0096 s, the neighbour's hold, in 2 of 20 rounds; only kept-alive test forks reach it; a pidfd waited on beside the report socket would end the wait"}]
```

Resolution: no introduced defect. The pre-existing out-of-scope finding at :869 is decided at wave close (candidate story: wait on the forwarder's pidfd beside the report socket).
