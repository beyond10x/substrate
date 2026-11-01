---
format: aep.planning-md/3
id: review-result:wave-2026-11-01-d354-u1-adversary-pass-1
kind: review-result
status: active
title: D354 U1 adversary pass 1
relations:
- reviews: story:namespace-no-egress-observed-on-every-node
revision: 1
---
# Adversary pass 1 — wave D354 U1 (story:namespace-no-egress-observed-on-every-node)

Diff `91c8163a...804d433c`, tree `unit/d354-u1` at `628dc1ca` plus one red case left uncommitted.
Agent `aep:adversary` (opus). Report as returned, header and findings verbatim; prose trimmed to the measured facts.

```
verdict: red
cases: executed 146→150, red 1
origin: introduced 1, pre-existing 1, undecided 0 (adversary)
```

Cases (all in `mod d354_adv`):

| case | asserts | now |
|---|---|---|
| `exec_tests::d354_adv::a_probe_published_elsewhere_during_the_policy_reread_refuses_the_start` | a later probe on node-b is published while the start waits on the policy re-read; keeper on node-a; the start is refused with no Pod | red, uncommitted |
| `exec_tests::d354_adv::a_stale_snapshot_is_capability_stale_before_the_node_is_compared` | an unpublished snapshot is `exec.capability-stale` even when the published node differs; no Pod | green, committed |
| `probe_tests::d354_adv::a_later_run_that_fails_after_its_keeper_node_was_read_clears_the_published_node` | probe 2 reads its node then fails `LimitsUnapplied`; the published node is `None` | green, committed |
| `probe_tests::d354_adv::a_lapsed_proof_publishes_no_node` | past `valid_until` the published node is `None` | green, committed |

Red run alone: `panicked at crates/substrate-kubernetes/src/exec_tests.rs:2153:45: story:namespace-no-egress-observed-on-every-node: the latest probe is on node-b and the keeper on node-a, yet the start was dispatched to node-a` — `test result: FAILED. 0 passed; 1 failed; ... 149 filtered out`.
Suite: `cargo test -p b10x-substrate-kubernetes --lib` — `test result: FAILED. 149 passed; 1 failed`.

Findings:

- Race on the probe node during the policy re-read. Measured: `exec.rs:261` reads `(generation, probed)` before `check_binding(...).await`; the comparison at `exec.rs:263` uses that value without re-reading `published`. What reaches it: probes run in their own task, and every daemon request calls `machine()`, which republishes (`operations.rs:448-449` on every start); the window is one API round trip. Fix: re-read the published node after `check_binding`. Risk, introduced.
- Stale tuple published over a newer one. Measured: `lib.rs:96-101` computes `capability_snapshot()` under the probe lock, releases it, then publishes under the execs lock; two interleaving `machine()` calls can publish the older (snapshot, node). The same mechanism exists at base for snapshot and generation. No case; reachable only by calling publish by hand, unproven. Risk, pre-existing.

Attacked and not broken: empty-string node (filtered on both sides); a probe failing before its run clears the node; a probe outliving its Lease is dropped; takeover and `forget_probe` clear the node; node absent from digest and wire; `Unserved` maps to 501 (`operations.rs:1785`); capability-stale precedes the node comparison.


Findings block as corrected by its author after the store refused the first two:

```findings
[
  {"file": "crates/substrate-kubernetes/src/exec.rs", "line": 263, "category": "concurrency", "severity": "warning", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "scope in; case exec_tests::d354_adv::a_probe_published_elsewhere_during_the_policy_reread_refuses_the_start (red, uncommitted): the probe node is read before the check_binding await and compared after it without a re-read, so a start can still be dispatched to the old node after a later probe on another node was published"},
  {"file": "crates/substrate-kubernetes/src/lib.rs", "line": 96, "category": "concurrency", "severity": "note", "verdict": "INFEASIBLE", "origin": "pre-existing", "message": "scope in; case none (the interleaving can only be built by hand, so no caller was shown to reach it): concurrent machine() calls compute the snapshot outside the published lock and can publish an older (snapshot, node) tuple over a newer one"},
  {"file": "crates/substrate-kubernetes/src/namespace/probe.rs", "line": 493, "category": "mutant", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "scope in; no defect, a guard that holds; case namespace::probe_tests::d354_adv::a_lapsed_proof_publishes_no_node (green, committed 628dc1ca): the node is taken from the expiry-filtered proof and is cleared by a later failed run and by a lapsed proof"},
  {"file": "crates/substrate-kubernetes/src/exec.rs", "line": 261, "category": "acceptance", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "scope in; no defect, a guard that holds; case exec_tests::d354_adv::a_stale_snapshot_is_capability_stale_before_the_node_is_compared (green, committed 628dc1ca): a stale snapshot is refused capability-stale before the node comparison, the order the 2026-11-01 amendment sets"}
]
```

Resolution: the introduced finding is fixed by `b83df354` (the red case is green, kubernetes --lib 150/0), merged into the integration branch at `244dfdce`. The pre-existing finding at lib.rs:96 is left for the wave close.
