---
format: aep.planning-md/3
id: review-result:wave-2026-11-02-d364-u4-security-pass-1
kind: review-result
status: active
title: 'D364 U4 security pass 1: a written policy is released only when shown stopped'
relations:
- reviews: story:written-policy-swept-when-shown-stopped
- reviews: story:kept-written-policy-log-names-the-write
revision: 1
---
# Security pass 1 — U4 of wave D364

Unit `story:written-policy-swept-when-shown-stopped` with `story:kept-written-policy-log-names-the-write`,
diff `8b9818dc...2404260e` (merged into `wave/2026-11-02-d364` at `7746a5d5`). Report as returned by
the security reviewer (`aep:security-reviewer`, Opus); machine paths shortened to the unit's scratch.

```
unit: story:written-policy-swept-when-shown-stopped + story:kept-written-policy-log-names-the-write; diff 8b9818dc...2404260e, tree unit/d364-u4 at 47d79cdf
verdict: nothing found
cases: executed 53→55, red 0
origin: introduced 0 / pre-existing 0 / undecided 0
wrote-outside-worktree: 6 paths (the unit's security scratch)
needs-coordinator: none
```

Invariant checked: a policy written after its create is released only when the replay proves its
container stopped. Two cases added in `crates/substrate-host/src/confinement/pod_cleanup_tests.rs`,
`mod d364_sec`, committed as `47d79cdf` (merged at `16fa0df0`), both green:

- `open_keeps_a_written_policy_on_every_replay_that_shows_no_stop`: an old written policy whose Pod
  is gone, against every fake replay that shows no stop (ForceDeleted, Gone 410, Expired, NeverCreated
  as a lone BOOKMARK, Ended, BoundDeleted, Added, Unavailable 503, Silent). The policy is never
  deleted, exactly one watch runs from the policy's own `resourceVersion`, and only Unavailable is
  deferred.
- `the_deferred_sweep_keeps_a_written_policy_on_every_replay_that_shows_no_stop`: a young written
  policy is deferred without a replay, aged to an hour, then `resweep` runs over the same replays
  except Silent, with the same three assertions.

```
test confinement::pod::cleanup_tests::d364_sec::the_deferred_sweep_keeps_a_written_policy_on_every_replay_that_shows_no_stop ... ok
test confinement::pod::cleanup_tests::d364_sec::open_keeps_a_written_policy_on_every_replay_that_shows_no_stop ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 432 filtered out; finished in 8.18s
```

Suite `test -p b10x-substrate-host --lib confinement::pod`: 55 passed, 0 failed (53 with
`--skip d364_sec`). `fmt --check` and clippy `--all-targets -D warnings` exit 0.

Why it holds: the `written` flag changes one branch (`gone.rs:411-417`), where a BOOKMARK before any
event of the Pod's name reads as unproven instead of never created. Every other outcome comes from
the unchanged `decide` (`gone.rs:171-212`), which releases a policy only when the run's container is
shown terminated in a Pod with `restartPolicy: Never`, or the Pod was deleted without ever being
bound to a node.

Checked and cleared: callers of `PodTrace::since` and `Default` see `written` false; `resweep` keeps
the flag (the deferred trace is cloned whole); a written policy whose Pod a read showed takes the
`pod_end` path, which has no BOOKMARK rule; a policy without `resourceVersion` is still kept before
any replay; the removed info line in `sweep_page` ran before any decision. Acceptance A is met by
`pod.rs:565-575` with `gone.rs:412`; acceptance B by the warns at `pod.rs:481` and `735` carrying
`WRITTEN_AFTER_CREATE`, emitted only when the policy is old and its Pod is gone.

```findings
[]
```
