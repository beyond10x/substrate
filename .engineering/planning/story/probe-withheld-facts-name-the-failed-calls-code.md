---
format: aep.planning-md/3
id: story:probe-withheld-facts-name-the-failed-calls-code
kind: story
status: implemented
title: Every probe fact the namespace driver withholds says why, with the failed call's own code
relations:
- informed_by: review-result:wave-2026-10-31-d337-adversary-pass-1
- informed_by: approval-record:d338-review-findings-decided-in-session
- informed_by: approval-record:d339-substrate-wave-2026-11-01
scope:
- confidence: cited
  path: crates/substrate-kubernetes/src/namespace/probe.rs
- confidence: cited
  path: crates/substrate-kubernetes/src/namespace/probe_tests.rs
revision: 7
transitions:
- {from: "draft", to: "proposed", at: "2026-11-01T00:28:53Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-01T00:37:23Z", actor: "human:timo", revision: 6}
- {from: "active", to: "implemented", at: "2026-11-01T03:39:31Z", actor: "human:timo", revision: 7, decided_on: {"recorded":{"test_result":1,"review_outcome":1}}}
---
# Story: every probe fact the namespace driver withholds says why, with the failed call's own code

## Outcome

Whenever the namespace driver's probe withholds a fact, its `warn` names the cause in one form: the
reason code of the call that failed, as that call reported it (`kubernetes.api-failed` only for an
API refusal or loss, `workspace.capacity`, `state.store-failed` otherwise), and
"`<fact>` was not proved" for each fact withheld.

## Evidence

Out-of-scope rows of `review-result:wave-2026-10-31-d337-adversary-pass-1`, adopted under D338:

- `create_probe_object` (`crates/substrate-kubernetes/src/namespace/probe.rs`, the PVC and keeper
  creates) tags every create failure `kubernetes.api-failed`, `workspace.capacity` and
  `state.store-failed` included. Pre-existing; D337 fixed the same mistake for the probe Pod only.
- `scheduled_node` and `ended`: a refused `pods get` ends the run with only the fact lines and no
  `code=`.
- The NetworkPolicy check that withholds `confinement.no-egress` and `heartbeat_stood_still`, which
  withholds `confinement.whole-tree-kill`, warn in their own wording, not
  "`<fact>` was not proved".

## Design

Carry the `DriverError` code through `create_probe_object`'s refusal instead of `API_FAILED`; give
`scheduled_node` and `ended` a `RunFailed` with `kubernetes.api-failed` when the GET was refused or
lost; reword the two property warnings to the shared form, keeping their reasons after a colon.

## Acceptance

- A ledger that fails `bind_uid` for the probe's PVC or keeper yields a startup refusal naming
  `state.store-failed`, not `kubernetes.api-failed`.
- A refused `pods get` while the probe waits for its Pod logs `code="kubernetes.api-failed"` and the
  ten fact lines.
- A probe whose bound NetworkPolicy allows egress, and one whose heartbeat moves, each log
  "`<fact>` was not proved" for the fact withheld.

## Acceptance added by D339

- Added 2026-11-01 (D339, for `story:namespace-lane-verb-case-accepts-only-its-own-refusal`): the
  probe's removal warnings (`remove_probe_row` and the leftover removal) and the run's refused
  `pods get` in `scheduled_node` and `ended` carry the API's answer, so a refusal of `pods get`,
  `pods delete` or `persistentvolumeclaims get` names `cannot <verb> resource "<resource>"` on a
  `WARN` line.
