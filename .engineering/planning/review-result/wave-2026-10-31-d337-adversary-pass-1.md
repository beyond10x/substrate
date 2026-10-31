---
format: aep.planning-md/3
id: review-result:wave-2026-10-31-d337-adversary-pass-1
kind: review-result
status: active
title: 'Wave 2026-10-31 D337 (namespace lane S6a): adversary pass 1'
relations:
- reviews: approval-record:d337-namespace-lane-s6-s7-fix-wave
revision: 1
---
# Adversary review, wave 2026-10-31 D337 (namespace lane S6a), pass 1

Reviewer: aep:adversary, tree `substrate-d337-adv`, range `af4d9f78...08ee7733`, diff against the
D337 S6a acceptance only ("the probe logs a reason code (kubernetes.api-failed or "<fact> was not
proved") when the pod-log read is refused, failing test first"). Its two cases are carried onto
the wave branch uncommitted by the reviewer and committed with the fix.

```
unit: substrate D337 S6a, range af4d9f78...08ee7733 at 08ee7733
verdict: CONFIRMED (one red case)
cases: executed 125 to 127, red 1
origin: introduced 2 / pre-existing 0 / undecided 0
needs-coordinator: none
```

Suite: `cargo test -p b10x-substrate-kubernetes --lib`: `test result: FAILED. 126 passed; 1 failed`
(`namespace::probe_tests::adversary_d337::a_store_failure_binding_the_probe_pod_is_not_named_kubernetes_api_failed`),
exit 101. After the fix: `test result: ok. 127 passed`.

Attacked and not broken: every route to an unreadable `pods/log` read maps to
`kubernetes.api-failed`; a refused probe Pod create names it with all ten fact lines (green case
`a_refused_probe_pod_create_names_its_reason_and_each_unproved_fact`); `RUN_FACTS` is exactly the
ten facts the run derives, each its wire key; the published facts are unchanged; the lane's
parsers read the daemon's ANSI-coloured lines; the per-thread log capture held with three capture
tests running at once.

```findings
[
{"file":"crates/substrate-kubernetes/src/namespace/probe.rs","line":652,"category":"acceptance","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"A probe Pod create that fails for a non-API reason (state.store-failed from the ledger, or workspace.capacity from a quota) was logged code=kubernetes.api-failed because RunFailed::api discarded error.code. Fixed in the wave: the arm keeps error.code."},
{"file":"xtask/src/namespace_lane_run.rs","line":1751,"category":"judgement","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"The per-fact warnings written on every RunFailed, non-API failures included, let verb_refusal accept a Role-verb case whose probe run failed for a reason unrelated to the removed verb; no state found that forces it."}
]
```

## Out of scope

One row each, for the next triage sheet; none is a story.

| finding | evidence |
|---|---|
| S6b: the namespace lane's workspace create for "same-named object of another deployment" answered 500 `operation.outcome-unknown` in 1 of 3 runs; cause not observed (S7 followed from it) | failed in the four-lane run at `620e89ce` (log `~/.cache/substrate-d326/lanes3/namespace.log`); passed at `3b3998c8` (D332 trace run 1) and in the D337 traced run at `620e89ce` (daemon log `~/.cache/substrate-d326/lanes-d337/`) |
| `create_probe_object` (PVC and keeper creates) tags every create failure `kubernetes.api-failed`, `workspace.capacity` and `state.store-failed` included; pre-existing | `crates/substrate-kubernetes/src/namespace/probe.rs`, `create_probe_object` |
| `verb_refusal` accepts any `code=` anywhere in the daemon log, related to the removed verb or not; pre-existing | `xtask/src/namespace_lane_run.rs:360-369` |
| `scheduled_node` and `ended`: a refused `pods get` ends the run with no `code=`, only the fact lines | `probe.rs`, `scheduled_node`, `ended` |
| `FakeCluster::refuse_logs` answers reason `Forbidden` whatever the status, and only after the Pod-exists check (real RBAC refuses first) | `crates/substrate-kubernetes-client/src/fake.rs`, `refuse_logs`, `log` |
| The NetworkPolicy and heartbeat warnings that withhold no-egress and whole-tree-kill do not use the "`<fact>` was not proved" form | `probe.rs`, `snapshot` NetworkPolicy check and `heartbeat_stood_still` |
