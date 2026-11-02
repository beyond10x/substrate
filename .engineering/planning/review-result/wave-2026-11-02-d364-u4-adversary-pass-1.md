---
format: aep.planning-md/3
id: review-result:wave-2026-11-02-d364-u4-adversary-pass-1
kind: review-result
status: active
title: 'D364 U4 adversary pass 1: written policy sweep and its kept log'
relations:
- reviews: story:written-policy-swept-when-shown-stopped
- reviews: story:kept-written-policy-log-names-the-write
revision: 1
---
# Adversary pass 1 — U4 of wave D364

Unit `story:written-policy-swept-when-shown-stopped` with `story:kept-written-policy-log-names-the-write`,
implementation `8b9818dc...2404260e` (merged into `wave/2026-11-02-d364` at `7746a5d5`). Report as
returned by the adversary (`aep:adversary`, Opus); machine paths shortened to the unit's scratch.

```
unit: story:written-policy-swept-when-shown-stopped, story:kept-written-policy-log-names-the-write (U4 of D364), unit/d364-u4 at 9398f0eb
verdict: green (nothing found)
cases: executed 55→60, red 0
origin: introduced 0, pre-existing 0, undecided 0
wrote-outside-worktree: the unit's adversary scratch (9 files)
needs-coordinator: no
```

Five cases added in `crates/substrate-host/src/confinement/pod_cleanup_never_created.rs`,
`mod d364_adv` nested in `mod d364` (one global capture subscriber), commit `9398f0eb`, all green:

- `a_written_policy_not_shown_stopped_is_kept`: kept by open and by resweep under ForceDeleted, Gone,
  Expired, Ended, BoundDeleted and Added; each replay starts once from `resourceVersion=1`.
- `a_written_policy_deferred_unanswered_keeps_its_write`: open defers on a 503; in resweep a lone
  `BOOKMARK` keeps the policy with exactly one warn whose `why` names the write, and Terminated
  sweeps it with no warn.
- `each_kept_written_policy_logs_its_own_one_warn`: across two pages, each policy has exactly one
  line naming the write, and it is the WARN.
- `an_unwritten_kept_policy_does_not_name_the_write`: under Gone and BoundDeleted, one warn and no
  line names the write.
- `a_young_written_policy_in_the_deferred_sweep_logs_nothing`: a young policy in resweep is not
  replayed, stays deferred, and nothing names the write.

A removed probe showed the capture would see the old info line and that a `PodTrace::since(None)`
trace fails the B predicate, so both named mutants would turn the B cases red (measured on the
capture, not on a mutated `sweep_page`).

Runs: `--lib d364_adv` 6 passed (5 cases plus the probe, before its removal); `--lib confinement::pod` 55 before
(`--skip d364_adv`) and 60 after; `-p b10x-substrate-host --lib` 427 passed, 12 ignored;
`-p b10x-substrate-kubernetes-client --lib` 53 passed; fmt and clippy `--all-targets -D warnings`
exit 0.

Attacked and could not break: the safe direction (a written policy is swept only on a replayed
terminated container under `restartPolicy: Never` or a deletion of a never-bound Pod); the unwritten
path (`PodTrace::since`, `pod.rs:573`); the written flag through an Unanswered deferral; other
`PodTrace::since` callers (`exec.rs:180,676`, `pod.rs:1313`); one warn per policy per sweep and none
for a young policy or a present Pod; the capture subscriber cannot change an unrelated test. A
written policy kept for a 410 or an added Pod gets a warn naming that cause, not the write; that is
within story B's Outcome, which concerns a policy kept because of the write.

```findings
[]
```
