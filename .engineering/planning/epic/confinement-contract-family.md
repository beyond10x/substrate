---
format: aep.planning-md/3
id: epic:confinement-contract-family
kind: epic
status: active
title: Confinement is one contract family with an adapter per mechanism (atlas ADR 0109)
summary: One ConfinementProfile and ConfinementAdapter in substrate-host; six adapters each declare the contract families they serve as capability facts; a missing family is refused by name.
owner: substrate
tags:
- adr-0109
- confinement
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-28T13:33:38Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"review_outcome":2}}}
- {from: "proposed", to: "active", at: "2026-10-28T13:33:39Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"review_outcome":2}}}
---
# Epic: Confinement is one contract family with an adapter per mechanism

## Outcome

Substrate is the ground an agent stands on: workspaces, filesystem access, networking, their
confinement, auditing and limits, behind one `ConfinementProfile` and one `ConfinementAdapter`
trait in `b10x-substrate-host`. Six adapters implement it: `local` (declared insecure), `linux`
(namespaces, bubblewrap, cgroup v2, seccomp: the driver that exists), `macos` (Seatbelt, ADR 0037),
`systemd` (a delegated scope as the bounds provider), `oci` (Docker or podman) and `pod`
(Kubernetes). Each adapter declares, as capability facts proved by probe at open, the subset of the
contract families it serves: process run, process watch, output read, wait, workspace, filesystem
view, network egress, network ingress, limits and audit. A client reads those facts before it asks;
a profile that needs a family or a fact the chosen adapter lacks is refused with that family's or
fact's name, never degraded (invariants 3 and 4).

## Decision this epic carries out

Atlas ADR 0109, "Confinement is one Substrate contract with an adapter per mechanism", accepted
2026-10-27 and on atlas `main` at `4a2bcf13`
(`architecture/adr/0109-confinement-is-one-substrate-contract-with-adapters.md`). Decision 8 adds
the contract families and the per-adapter subset; Decision 9 takes F1 B (the first contract ships
what Substrate enforces today; ingress routes and rate bounds are reserved fields), F2 A
(conductor's and autodev's own mechanisms move later, outside this epic) and F3 A (the host can
refuse `local` through a policy fact). The operator's words are quoted in
`approval-record:adr-0109-confinement-plan`. Design inputs, read-only:
`sandbox-layer.md` and `chain-scope.md` of the 2026-10-27 inventory (kept outside the repository).

## Order

0. `story:ess-pin-0-70-0` — the ESS pin moves to the newest release on its own, wire bytes
   unchanged.
1. `story:confinement-domain-specification` — the ESS domain `substrate.confinement` (profiles,
   families, adapters, capability facts, refusals, one scenario per promise and adapter) and
   Substrate ADR 0044 recording ADR 0109 locally (invariant 8: a contract change has its ADR before
   code).
2. `story:confinement-contract-on-linux-and-local` — S0: the trait, the profile, the `linux`
   adapter over the existing `HostDriver` and the `local` adapter, selected through
   substrate-host's own API. No wire change: `HostDriverKind` is wire format
   (`crates/substrate-wire/src/lib.rs:3368`), so naming adapters on the wire waits for a successor
   bundle.
3. One story per adapter: `story:macos-adapter-on-seatbelt` (S1) and
   `story:macos-seatbelt-policy-runs-cargo-test` (S2), `story:systemd-scope-bounds-provider`,
   `story:oci-adapter`, `story:pod-adapter`.
4. `story:confinement-consumer-surface` — the surface loom-sdk and takt call, proved by a
   consumer-written test and frozen by a committed list of its public items.
5. `story:sandbox-repository-parity` — the `sandbox` repository's bubblewrap and Docker settings
   covered by Substrate, proved by its mirror test moved here; a `b10x-substrate confine run` verb
   the O6 loop can switch to.

## Out of scope

- Any wire change: a successor bundle naming adapters and their facts on the wire
  (`HostDriverKind` variants, ADR 0037 decision 5) is the next epic's first story.
- Filling the reserved ingress-route and rate-bound fields (ADR 0109 F1 B): refused by name here.
- Downstream hops: loom L1, takt T1, mantle M1, Atlas's O6 loop switch (it still calls the
  `b10x-sandbox` binary), conductor/autodev (F2 A), and the `substrate` node with edges from loom
  and mantle in `agent-stack/stack.yaml` (ADR 0109 consequences; that file is the `agent-stack`
  repository's). Archiving `beyond10x/sandbox` is the operator's act.
- Plan 03's Docker *driver* behind the wire (`epic:container-driver-entry`) and the Kubernetes
  namespace *driver* (`epic:kubernetes-deployment-and-driver`): those serve the whole wire contract
  through the daemon; the `oci` and `pod` adapters here serve the confinement contract through
  substrate-host's API.

## Delivery

The operator, 2026-10-27: "do all changes against a single integration branch, provide 1 single
PR in the end". Every unit merges into one integration branch, and the epic ships as one pull
request. The release after that merge is a separate step under the coordinator's grant
("releases through substrate's own process", `approval-record:adr-0109-confinement-plan`) and
follows `AGENTS.md` § Releases, including the `Confinement-lane:` and `Quota-lane:` tag records;
no story claims it.

## Done when

Every story above is implemented, each adapter's conformance lane runs its scenarios from the
committed `spec/confinement-suite.json` on the platform that can prove it (Linux gate, delegated
lane, `macos-15` job, Docker lane, Kubernetes lane), and an absent prerequisite is reported as a
named refusal or an absent lane, never as a pass.
