---
format: aep.planning-md/3
id: story:confinement-adapter-on-the-wire
kind: story
status: implemented
title: The wire states what the confinement adapter proves, never its name
summary: 'Design 30: a successor bundle publishes confinement families and properties as facts, lets a start require them, and keeps driver as host.'
owner: substrate
tags:
- adr-0109
- confinement
- wire
relations:
- informed_by: epic:confinement-contract-family
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-31T11:27:59Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-31T11:28:48Z", actor: "human:timo", revision: 4}
- {from: "active", to: "implemented", at: "2026-10-31T16:30:11Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"test_result":1,"review_outcome":1}}}
---
# Story: the wire states what the confinement adapter proves, never its name

## Outcome

A client of the daemon chooses where to run by the confinement families and properties a daemon
proved, reads them from `/v1/machine`, and can require them on an exec or session start. The daemon
refuses an unmet requirement by the family's or fact's name, before dispatch. No adapter, mechanism
or deployment-profile name appears on the client wire, and `driver` stays `host`.

## Design

[Design 30](../../../docs/design/30-confinement-adapter-on-the-wire.md), status `accepted`
2026-10-31 with every recommended answer of its § 9, recorded in
[ADR 0048](../../../adr/0048-the-wire-states-what-a-confinement-adapter-proves-never-its-name.md)
(`approval-record:design-30-accepted`). The bundle is `0.26.0` (D10).

This is the first story of the epic after `epic:confinement-contract-family`, which declares
naming adapters on the wire out of its own scope ("the next epic's first story"). It is therefore
`informed_by` that epic rather than decomposing it.

## Acceptance

Under the recommended answers of design 30 § 9:

- A successor bundle (`0.25.0` or the next free version) exists under `contracts/substrate-wire/`,
  rendered by `cargo xtask render-bundle`. It adds no route, and its compatibility block pins its
  predecessor and route counts. It publishes:
  - the optional `confinement.family.*` and `confinement.*` property facts;
  - the optional `sandbox.requires` on both starts;
  - the optional `requires` and optional `cgroup` on the applied record;
  - `confinement.family-unserved` (501) and `confinement.fact-unserved` (501), each with a vector.

  `"driver": {"const": "host"}` is unchanged. `cargo xtask check-bundles` in `scripts/gate.sh`
  and `cargo xtask check-json` cover it.
- The daemon and SDK advertise that bundle. `driver_version` is asserted to carry no adapter or
  mechanism name.
- The portable lane of `crates/substrate-daemon/tests/runtime_vectors.rs` asserts each of the
  following:
  - the published family facts;
  - a `requires` naming an unserved family (`network-ingress`), refused
    `confinement.family-unserved` and recorded as refused;
  - an unproved property, refused `confinement.fact-unserved`;
  - an unknown name, refused `request.schema-invalid`.

  The delegated lane asserts `linux`'s expected set (design 30 § 4.3), and a run admitted under
  `requires` whose applied record echoes it.
- A daemon restarted on a different adapter publishes a new snapshot; a start naming the old one is
  `exec.capability-stale`.
- Every released bundle is byte for byte unchanged. The full gate is green.

## Boundaries

- No `HostDriverKind` variant, no `adapter` member and no profile name on the client wire.
- The daemon serves neither `local`, `oci` nor `pod` on the exec or session route.
- The macOS daemon (ADR 0037 step 2) is its own story; this one only makes its wire fit.
- Consumer repositories (harness, loom, takt, mantle) are not changed here.
- The reserved ingress-route and rate-bound fields stay reserved.

## Scope

Inferred from design 30 on 2026-10-28; not yet scoped by `story-scoper`.

- `crates/substrate-wire/src/lib.rs`: `CapabilityFacts`, `ConfinementRequest`,
  `AppliedConfinement`, `ADVERTISED_CONTRACT_BUNDLE`.
- `crates/substrate-daemon/src/app/operations.rs`: the `requires` check after
  `exec.capability-stale`.
- `crates/substrate-host/src/probe.rs`: the fact publication.
- `crates/b10x-substrate-sdk/src/model.rs`: `Machine` helpers.
- `xtask/bundle-source/<version>/` and the new `contracts/substrate-wire/<version>/`.
- `scripts/gate.sh`.
- `crates/substrate-daemon/tests/runtime_vectors.rs`.
- `AGENTS.md` invariant 6, `CHANGELOG.md`, and the website's fact table that `check-docs` derives.
- Under D7, ADR 0045's metric catalogue.
