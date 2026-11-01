---
format: aep.planning-md/3
id: story:namespace-lane-verb-case-accepts-only-its-own-refusal
kind: story
status: implemented
title: The namespace lane's Role-verb case accepts only a refusal that follows from the removed verb
relations:
- informed_by: review-result:wave-2026-10-31-d337-adversary-pass-1
- informed_by: approval-record:d338-review-findings-decided-in-session
- depends_on: story:probe-withheld-facts-name-the-failed-calls-code
- informed_by: approval-record:d339-substrate-wave-2026-11-01
scope:
- confidence: cited
  path: xtask/src/namespace_lane_run.rs
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-11-01T00:28:54Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-01T00:37:23Z", actor: "human:timo", revision: 5}
- {from: "active", to: "implemented", at: "2026-11-01T03:39:31Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"test_result":1,"review_outcome":1}}}
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

## Design decided by D339

Decided 2026-11-01 (D339, from the scoping): each verb case's daemon log is created after
`set_role`, so every byte is already written after the Role change. The rule is lane-only:
`verb_refusal` accepts a refusal only from a `WARN` line that carries the API's answer
`cannot <verb> resource "<resource>"` for the removed verb (matched in both the `Debug`-escaped
`why=` form and the plain `message=` form; `pods` never matches `pods/log` or `pods/exec`), and it
decides only after the first probe has finished, never on facts absent because no probe ran yet.
A `since` offset, as `pending_destroy_codes` takes, lets a unit test show that an earlier line
refuses nothing. The route path (`pods list`, `persistentvolumeclaims delete`) keeps its own HTTP
answer. The five cases whose driver line carries a code only (`pods get`, `pods list`,
`pods delete`, `persistentvolumeclaims get`, `persistentvolumeclaims delete`) name the resource
once `story:probe-withheld-facts-name-the-failed-calls-code` carries the API's answer; this story
depends on it.
