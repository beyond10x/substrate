---
format: aep.planning-md/3
id: story:namespace-lane-verb-case-accepts-only-its-own-refusal
kind: story
status: proposed
title: The namespace lane's Role-verb case accepts only a refusal that follows from the removed verb
relations:
- informed_by: review-result:wave-2026-10-31-d337-adversary-pass-1
- informed_by: approval-record:d338-review-findings-decided-in-session
revision: 2
transitions:
- {from: "draft", to: "proposed", at: "2026-11-01T00:28:54Z", actor: "human:timo", revision: 2}
---
# Story: the namespace lane's Role-verb case accepts only a refusal that follows from the removed verb

## Outcome

A Role-verb case of `cargo xtask namespace-lane observe` passes only when the daemon's log names a
refusal caused after the verb was removed: a `code=` or "`<fact>` was not proved" line written
after the Role changed, on a line that carries the API's answer for that resource. A run that
failed for an unrelated reason no longer passes the case.

## Evidence

From `review-result:wave-2026-10-31-d337-adversary-pass-1`, adopted under D338:

- Out-of-scope row: `verb_refusal` (`xtask/src/namespace_lane_run.rs:360-369`) accepts any `code=`
  anywhere in the whole daemon log, related to the removed verb or not. Pre-existing.
- INFEASIBLE note (introduced by D337): the per-fact warnings are written on every failed probe
  run, so a run that failed for a reason unrelated to the removed verb satisfies the
  missing-and-unproved overlap.

## Design

Read only the log written after `set_role` (the lane already holds the offset for pending destroys,
`pending_destroy_codes`), and require the refusal line to name the removed resource
(`pods/log`, `pods/exec`, `networkpolicies`, …) in the API's answer it carries.

## Acceptance

- A log whose only code predates the Role change refuses nothing.
- A log whose failed run names another resource's refusal refuses nothing.
- The observed D337 `pods/log` lines still refuse the `pods/log` case.
