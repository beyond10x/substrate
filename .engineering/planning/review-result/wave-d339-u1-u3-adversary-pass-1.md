---
format: aep.planning-md/3
id: review-result:wave-d339-u1-u3-adversary-pass-1
kind: review-result
status: active
title: Adversary pass 1 of wave D339, units U1 to U3
relations:
- reviews: story:probe-withheld-facts-name-the-failed-calls-code
- reviews: story:namespace-lane-verb-case-accepts-only-its-own-refusal
- reviews: story:exec-end-after-await-never-panics
revision: 1
---
# Adversary pass 1 of wave D339, units U1 to U3

Agent type `aep:adversary`, against each story's Acceptance only (operator rule of 2026-10-31).
Tree `substrate-d339-adv` at the wave head `82ee7f92`, diff `f6d02148..82ee7f92`. Two cases
written; `cargo test -p b10x-substrate-kubernetes --lib` with them: 135 passed, 2 failed; `cargo
test -p xtask`: 456 passed, 9 ignored.

```findings
- file: crates/substrate-kubernetes/src/namespace/workspace.rs
  line: 449
  category: acceptance
  severity: warning
  verdict: CONFIRMED
  origin: pre-existing
  message: A probe PVC or keeper whose create answer was lost and whose adopting bind_uid the ledger fails is refused kubernetes.api-failed, not state.store-failed, because the arm discards resolve's store error.
- file: crates/substrate-kubernetes/src/exec.rs
  line: 979
  category: acceptance
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: In the Acceptance's race (one observation ends and acknowledges, the other awaits its GET) the driver answers the late observation with its own decided end, not the recorded end.
```

Held: a refused `pods get` in `scheduled_node` and `ended`, `pods delete` and
`persistentvolumeclaims get` in `remove_owned` each carry the API's answer on a `WARN` line; 408
and 429 are retried. `verb_refusal` run unchanged over the real D337 lane daemon log refuses only
`pods/log get` and nothing before "probe ran"; `pods` never matches `pods/log`. The
not-acknowledged race returns the recorded end; no `unreachable!` is left in `exec.rs`.

Out of scope (not stories, decided by the coordinator under D338): the late path in `exec.rs`
ignores `settle`'s output failure and does not queue the Pod for garbage collection;
`verb_refusal` counts a "probe ran" line of a run whose result a Lease-epoch change dropped and
matches `WARN` anywhere in a line; `MAX_MESSAGE_CHARS` 300 may cut `cannot <verb> resource` for
very long names.

Coordinator decision (D339, 2026-11-01):

- `workspace.rs:449`: fixed in the wave (`b0c433d4`): the arm keeps `resolve`'s own error, as the
  409 arm does; the case is green, lib 136 passed.
- `exec.rs:979`: no change. The acknowledged race ends at the daemon: the exec is forgotten only
  after the store recorded its end, and the late observation's `put_exec` answers `Superseded`
  with the stored record, which `exec_observe` serves (`crates/substrate-daemon/src/app/execs.rs`,
  the `Superseded` arm), so the caller receives the recorded end. The driver cannot hold an end it
  was told to forget; the store's terminal-over-terminal rule is held by
  `crates/substrate-store/src/tests.rs` (`Superseded(winner)`). The driver-level case is dropped.
- The out-of-scope rows: no stories. The late path's two items are moot once the store answers
  `Superseded`; the lane items concern a test harness whose cases pass on real logs.
