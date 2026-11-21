---
format: aep.planning-md/3
id: story:confinement-profile-ingress-routes
kind: story
status: draft
title: A confinement profile declares the ingress a run serves
summary: ingress_routes becomes a typed field served by the linux adapter over the inbound-endpoint mechanism; an ADR supersedes ADR 0044 decision 5 for it.
owner: substrate
tags:
- confinement
- network
- sdc
relations:
- decomposes: epic:runs-are-governed-and-limit-hits-are-events
- depends_on: story:inbound-endpoint-for-a-confined-exec
- depends_on: story:executable-lists-on-oci-and-pod
- depends_on: story:exec-observation-reports-process-count-hits
- depends_on: story:exec-file-change-record-on-the-wire
scope:
- confidence: inferred
  path: AGENTS.md
- confidence: inferred
  path: adr
- confidence: inferred
  path: adr/README.md
- confidence: inferred
  path: crates/substrate-daemon/tests/runtime_vectors.rs
- confidence: cited
  path: crates/substrate-host/src/confinement/facts.rs
- confidence: cited
  path: crates/substrate-host/src/confinement/linux.rs
- confidence: cited
  path: crates/substrate-host/src/confinement/mod.rs
- confidence: cited
  path: crates/substrate-host/src/confinement/profile.rs
- confidence: cited
  path: crates/substrate-host/src/probe.rs
- confidence: cited
  path: crates/substrate-host/tests/confinement-public-items.txt
- confidence: inferred
  path: spec/confinement-suite.json
- confidence: cited
  path: spec/confinement/confinement.yaml
- confidence: inferred
  path: spec/confinement/scenarios
- confidence: inferred
  path: website/docs
- confidence: cited
  path: website/docs/reference/contract.md
revision: 10
---
# Story: A confinement profile declares the ingress a run serves

## Outcome

`ConfinementProfile.ingress_routes` is a typed field. A profile opened on the `linux` adapter that
names one inbound endpoint gets it served, the adapter serves the family `network_ingress`, and a
connection from a peer the route does not allow is refused. The daemon publishes
`confinement.family.network-ingress` from the same proof. An adapter that does not serve the
family still refuses the profile by the family's name.

## Why

- `ingress_routes` is reserved: any value is refused `confinement.reserved-field-unserved`
  (`crates/substrate-host/src/confinement/profile.rs:93-94`,
  `crates/substrate-host/src/confinement/mod.rs:549-553`). The specification marks its shape
  unmapped (`spec/confinement/confinement.yaml:319-320`).
- `network_ingress` is served by no adapter (`crates/substrate-host/src/confinement/linux.rs:29`).
- The daemon's family fact is set to `None` (`crates/substrate-host/src/probe.rs:294-295`), and
  the test `the_published_set_is_the_linux_adapters_proof` holds the published set equal to the
  `linux` adapter's proof (`crates/substrate-host/src/probe.rs:1146-1218`). So the adapter cannot
  serve the family without the daemon publishing it.
- ADR 0044 decision 5 reserves ingress routes and rate bounds
  (`adr/0044-confinement-is-a-contract-family-served-by-declared-adapters.md:48`), following atlas
  ADR 0109 F1 B.
- `story:inbound-endpoint-for-a-confined-exec` (draft) decides the mechanism for the wire and the
  host driver. This story is the in-process field over that mechanism.
- The relayed requirement of 2026-11-21 names "egress and ingress allow-lists"
  (`epic:runs-are-governed-and-limit-hits-are-events`).

## What changes

1. **An ADR first** (`AGENTS.md` invariant 8), the next free number in `adr/` when the story
   starts, that supersedes ADR 0044 decision 5 for `ingress_routes` only. It states the route's
   shape (the declared endpoint and who may connect), takes the mechanism from the ADR of
   `story:inbound-endpoint-for-a-confined-exec`, and says whether atlas ADR 0109 F1 B needs a
   matching change in the `atlas` repository.
2. `spec/confinement/confinement.yaml` types the field, removes its reserved-field refusal and
   keeps `rate_bounds` reserved; `cargo xtask check-ess` regenerates the suite.
3. The `linux` adapter serves `network_ingress` where the host driver's endpoint fact is proved
   (`linux::proof`, `crates/substrate-host/src/confinement/linux.rs:166`).
4. `publish_confinement` publishes `confinement.family.network-ingress` from the same clause, and
   the probe test and the runtime vectors' family list change with it. Today the delegated lane
   publishes "every family but `network-ingress`" and refuses a start that requires it
   `confinement.family-unserved` (`AGENTS.md` § The gate).

## Acceptance

In the delegated lane, a run on the `linux` adapter whose profile declares one inbound endpoint
answers a connection from an allowed peer, and a connection from a peer outside the route's
allow-list is refused; before this story the same profile is refused
`confinement.reserved-field-unserved`.

Checks:

- The ADR is accepted and indexed in `adr/README.md` before any code of this story.
- A profile with `ingress_routes` on `local`, `macos`, `oci`, `pod` or `systemd` around `local`
  is refused `confinement.family-unserved` naming `network_ingress`. `systemd` around `linux`
  gives `linux`'s answer.
- `rate_bounds` set to any value is still refused `confinement.reserved-field-unserved`.
- The run still cannot open an outbound connection to an undeclared destination.
- `the_published_set_is_the_linux_adapters_proof` passes with `network-ingress` in both sets on a
  host that proved the endpoint fact, and the delegated runtime vectors assert the family
  published and a start requiring it admitted. The portable lane still refuses that start
  `confinement.family-unserved`.
- `crates/substrate-host/tests/confinement-public-items.txt` is regenerated in the same change.

## Boundaries

- The mechanism, the wire field and the bundle: `story:inbound-endpoint-for-a-confined-exec`.
- `rate_bounds`: not filled here.

## Order

After `story:inbound-endpoint-for-a-confined-exec`: it uses that story's mechanism. After
`story:executable-lists-on-oci-and-pod` and
`story:exec-observation-reports-process-count-hits` only because of shared files:
`spec/confinement/confinement.yaml` with the first,
`crates/substrate-daemon/tests/runtime_vectors.rs` with the second. After
`story:exec-file-change-record-on-the-wire` only because both change
`crates/substrate-daemon/tests/runtime_vectors.rs`, `crates/substrate-host/src/probe.rs` and
`AGENTS.md`. Through these edges it is after every other story that changes `profile.rs`,
`facts.rs`, `mod.rs`, `linux.rs` or `probe.rs`, including `story:run-observation-carries-usage`
and `story:exec-file-changes-are-recorded`, which are not part of the epic.

## Scope

- `adr` (a new ADR), `adr/README.md`. Inferred.
- `spec/confinement/confinement.yaml`. Cited: `:319-327`.
- `spec/confinement/scenarios`, `spec/confinement-suite.json`. Inferred.
- `crates/substrate-host/src/confinement/profile.rs`. Cited: `:93-96`.
- `crates/substrate-host/src/confinement/mod.rs`. Cited: `:549-557`.
- `crates/substrate-host/src/confinement/facts.rs`. Cited: `:65`.
- `crates/substrate-host/src/confinement/linux.rs`. Cited: `:29`, `:166`, `:177-197`.
- `crates/substrate-host/src/probe.rs`. Cited: `:294-295`, `:1146-1218`.
- `crates/substrate-daemon/tests/runtime_vectors.rs`. Inferred.
- `crates/substrate-host/tests/confinement-public-items.txt`. Cited: `:1-3`.
- `AGENTS.md`. Inferred: it states the delegated lane's family list.
- `website/docs/reference/contract.md`. Cited: `:172` says the daemon never publishes
  `network-ingress`.
- `website/docs`. Inferred: `cargo xtask check-docs` holds the site's contract tables to the spec.
