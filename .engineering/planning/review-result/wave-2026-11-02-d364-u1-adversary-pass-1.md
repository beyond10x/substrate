---
format: aep.planning-md/3
id: review-result:wave-2026-11-02-d364-u1-adversary-pass-1
kind: review-result
status: active
title: 'D364 U1 adversary pass 1: concurrent snapshots'
relations:
- reviews: story:concurrent-snapshots-never-publish-an-older-probe
revision: 1
---
# Adversary pass 1 — U1 of wave D364

Unit `story:concurrent-snapshots-never-publish-an-older-probe`, tree `substrate-d364-u1` at
`c88c3e00` (merged into `wave/2026-11-02-d364` at `2cf5e891`). Report as returned by the adversary
(`aep:adversary`, Opus); machine paths shortened to the unit's scratch.

```
unit: story:concurrent-snapshots-never-publish-an-older-probe (tree substrate-d364-u1, head c88c3e00 plus 1 uncommitted test hunk)
verdict: red
cases: executed 163→164, red 1
origin: introduced 1, pre-existing 0, undecided 0 (adversary)
wrote-outside-worktree: the unit's adversary scratch (red-case.log, suite.log)
needs-coordinator: no
```

Case added (red, uncommitted; `crates/substrate-kubernetes/src/exec_tests.rs` +32):
`exec_tests::d364_adv::a_newer_probe_that_fails_ends_the_interleave_instead_of_hanging_it`. It
calls `interleave::older_published_last` with a `newer_probe` closure that panics, and checks that
the call ends with that panic within 10 s. Measured: the call does not end; the assertion fails at
`exec_tests.rs:2275`, exit 101:

```
assertion `left == right` failed: story:concurrent-snapshots-never-publish-an-older-probe: a failing newer probe left interleave::older_published_last waiting on the older call for 10 s instead of returning the failure
  left: Err(Timeout)
 right: Ok(true)
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 163 filtered out; finished in 10.13s
```

Mechanism (`lib.rs:369-410`): the scope closure only borrows the `probed` sender, so it stays
alive in the outer frame. When `newer_probe` panics, `std::thread::scope` waits to join the older
thread, which blocks on `newer_probed.recv()` forever. What reaches it: the acceptance case in
`probe_tests.rs` d364 runs `.expect("the newer probe runs")` inside that closure, so a later
regression on the probe path would hang the suite instead of turning it red. Fix, not applied: move
`probed` into the closure, or drop it on unwind.

Suite run with the case: `cargo test -p b10x-substrate-kubernetes --lib` gave
`test result: FAILED. 163 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 20.50s`, EXIT=101.

Attacked, could not break: no deadlock under the new lock order (the publication lock is taken in
`machine`, `publish_exec_snapshot`, `published_exec_node` and `admitted_generation`, the last three
alone; binding, renewed and probe locks are never held while it is requested; the exec start holds
no guard at `admitted_generation`, `exec.rs:261`); a third call or a probe landing while the lock is
held cannot publish an older tuple; snapshot and node are compared after the interleave, generation
is equal in both tuples of the case but comes from the same `capability_snapshot` call; the seam is
`#[cfg(test)]` only, `published` widened to `pub(crate)`; on the old two-step publish both cases are
red (the implementor's red.log).

```findings
[{"file": "crates/substrate-kubernetes/src/lib.rs", "line": 394, "category": "boundary", "severity": "warning", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "scope in; case exec_tests::d364_adv::a_newer_probe_that_fails_ends_the_interleave_instead_of_hanging_it: when newer_probe panics, older_published_last never returns because the borrowed probed sender outlives the scope and the older thread blocks on newer_probed.recv(), so a probe regression hangs the acceptance case instead of failing it."}]
```
