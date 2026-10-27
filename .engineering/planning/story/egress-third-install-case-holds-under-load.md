---
format: aep.planning-md/3
id: story:egress-third-install-case-holds-under-load
kind: story
status: active
title: The egress third-install case holds under host load
relations:
- decomposes: epic:confinement-contract-family
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-10-27T07:24:39Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-27T07:24:39Z", actor: "human:timo", revision: 3}
---
## Outcome

`egress::tests::observed_inside::a_failed_third_install_leaves_no_earlier_listener_answering`
(`crates/substrate-host/src/egress.rs`) gives the same answer under host load as it does idle.

## Observed

On 2026-10-27, at the ADR 0109 integration head `9028aaf2`, the host library suite failed once
with `321 passed; 1 failed`, the failure being this case's panic at `egress.rs:3285` (the
assertion on the printed `127.0.0.x=refused` line). The 1-minute load average was above 12. Run
alone it passed, and the whole library suite passed `322 passed; 0 failed` on the next run.
`egress.rs` is not changed by the ADR 0109 work (its last change is `92a7ce7b`).

Inferred, not verified: the scripted sandbox's connection attempts race the refused set's
teardown, or `finish(Duration::from_secs(30))` returns before the script prints under load.

## Acceptance

The case passes 50 consecutive runs of the host library suite alongside a parallel build, or the
case is changed so its answer does not depend on timing, with the cause named in the commit.
