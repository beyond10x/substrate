---
format: aep.planning-md/3
id: review-result:wave-2026-10-30-u4-adversary-pass-1
kind: review-result
status: active
title: Adversary pass 1, unit u4 (an unconfirmed exec Pod deletion ends Unknown)
relations:
- reviews: story:unconfirmed-pod-deletion-ends-unknown
revision: 1
---
# Adversary pass 1, unit u4 (an unconfirmed exec Pod deletion ends Unknown)

Agent type `aep:adversary`. Tree on integration head `e83b69863` (unit commit `a1ce465cd`). Four
cases committed in `crates/substrate-kubernetes/src/exec_tests.rs` (`a34c3bfa`, merged as
`b56d76f8`): two red, two green pins (the garbage round itself ends an unconfirmed deletion Unknown;
a late confirmation never overwrites Unknown). Verdict: red.

```findings
- file: crates/substrate-kubernetes/src/exec.rs
  line: 448
  category: correctness
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: The 60 s bound is checked only when the exec is observed, so one history (uid present 60 s after the accepted kill, container shown ended at 65 s) ends Unknown when read at 61 s and Cancelled when first read at 70 s.
- file: crates/substrate-kubernetes/src/exec.rs
  line: 1041
  category: correctness
  severity: warning
  verdict: NEEDS-CHANGE
  origin: pre-existing
  message: A kill the API acted on (deletionTimestamp set under the bound uid) whose answer was lost never starts the bound, and the Kill intent suppresses the wall-bound deletion, so with no garbage round running the exec reads Running for good.
- file: crates/substrate-kubernetes/src/exec.rs
  line: 912
  category: robustness
  severity: note
  verdict: CONFIRMED
  origin: pre-existing
  message: The Unknown branch calls end after the GET await using a snapshot, so a concurrent finish and acknowledge in between reaches unreachable!; finish and gone share the window.
- file: crates/substrate-kubernetes/src/exec.rs
  line: 943
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: The acceptance says the kill intent stays recorded for the garbage round, but end clears it and the Pod is deleted again through the garbage set; equivalent behaviour, different wording.
```

Held: the 59 s against 60 s boundary on the injected clock; a delete accepted twice keeps the first
acceptance; a daemon restart inside the window ends the exec Unknown at once; an exec finished
before the delete; a Pod replaced by another uid during the window.

Coordinator decision: a bound-uid Pod present at or after acceptance plus 60 s ends Unknown before
any terminated-container reading; a kill intent with no recorded acceptance and a deletionTimestamp
on the bound-uid Pod records the acceptance at that observation's clock instant (fix unit on
`impl/kubernetes-collisions-metric-fix`). The `unreachable!` window becomes a follow-up story; the
acceptance wording is amended.
