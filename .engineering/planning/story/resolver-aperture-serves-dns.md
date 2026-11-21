---
format: aep.planning-md/3
id: story:resolver-aperture-serves-dns
kind: story
status: active
title: A declared resolver aperture serves DNS to a confined run
summary: A run that selects an operator-declared resolver aperture resolves names through it and nothing else; every other run keeps no resolver. An ADR amends ADR 0013 first.
owner: substrate
tags:
- loom
- network
relations:
- depends_on: story:aperture-reports-dial-outcome
scope:
- confidence: inferred
  path: AGENTS.md
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: README.md
- confidence: inferred
  path: adr
- confidence: inferred
  path: contracts/substrate-wire
- confidence: inferred
  path: crates/substrate-daemon/tests/runtime_vectors.rs
- confidence: cited
  path: crates/substrate-host/src/egress.rs
- confidence: inferred
  path: crates/substrate-host/src/probe.rs
- confidence: inferred
  path: crates/substrate-host/src/process.rs
- confidence: inferred
  path: crates/substrate-wire/src/lib.rs
- confidence: inferred
  path: scripts/gate.sh
- confidence: inferred
  path: xtask/bundle-source
- confidence: inferred
  path: xtask/src/bundle.rs
revision: 7
transitions:
- {from: "draft", to: "proposed", at: "2026-11-21T03:55:20Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"review_outcome":6}}, executor: "agent:substrate", correlation: "loom-accepts-aperture-stories-2026-11-21"}
- {from: "proposed", to: "active", at: "2026-11-21T03:55:20Z", actor: "human:timo", revision: 7, decided_on: {"recorded":{"review_outcome":6}}, executor: "agent:substrate", correlation: "loom-accepts-aperture-stories-2026-11-21"}
---
# Story: A declared resolver aperture serves DNS to a confined run

## Outcome

An operator declares a resolver as an aperture, and a run that selects it can resolve names
through it and through nothing else. A run that selects no resolver aperture still has no resolver
and performs no lookup, as today.

## Where the requirement comes from

Session `loom`, 2026-11-21: its `upstream-blocker:substrate-dns-resolver-aperture` holds Loom's
DNS story until a resolver can be reached through an aperture. Session `loom` is the stakeholder
of this story (`AGENTS.md` § Planning artifacts). If this story is declined, Loom's gate run keeps
refusing `net.*` operations.

## Why

Verified at `main` `638a7d70c`:

- A confined run has no resolver by decision: "The host is resolved once, at declaration, and
  pinned; the sandbox gets no resolver and performs no lookup" (`README.md` § Egress apertures;
  ADR 0013, `adr/0013-egress-apertures-are-declared-by-the-operator.md`).
- An aperture is declared `<name>=<host>:<port>/tcp` (`README.md` § Egress apertures). TCP is the
  only transport, and the name a child uses maps to loopback through a generated read-only
  `/etc/hosts`.
- Not established here: whether a TCP aperture to a resolver's port 53 already works for a child
  whose resolver configuration names the aperture's loopback address, and what the run's view
  holds at `/etc/resolv.conf`. The ADR records both from a probe.

## What changes

1. **An ADR first** (`AGENTS.md` invariant 8). ADR 0013 decides that a run has no resolver; this
   ADR amends that for a declared resolver aperture only. It decides:
   - whether a resolver aperture is an ordinary TCP aperture to port 53 plus a generated resolver
     configuration in the view, or a new aperture kind, and whether UDP is served;
   - what the run may ask: any name, or only names the operator lists. An open resolver is a
     channel out of the sandbox for whoever controls a zone, so the ADR states the exfiltration
     limit and the byte ceiling that bounds it (ADR 0014);
   - that a resolved address is of no use to the run by itself: it still reaches only declared
     apertures;
   - the capability fact, the applied record and each named refusal.
2. A successor contract bundle at the next free version when the story starts. The capability
   fact and the named refusals are published only in a bundle (invariant 6), so the bundle is
   certain; its check is added to `scripts/gate.sh`.
3. The host driver generates the resolver configuration for a run that selects the aperture and
   serves the relay.

## Acceptance

In the delegated lane, a run that selects a declared resolver aperture resolves a name served by a
test resolver on loopback with the system resolver (`getent hosts <name>`), with no resolver
configuration supplied by the caller; before this story the driver generates no resolver
configuration for any run, and the same command fails in the same run.

Checks:

- The ADR is accepted and indexed in `adr/README.md` before any code of this story.
- A run that selects no resolver aperture fails the same lookup.
- A run with the resolver aperture still cannot open a connection to an address it resolved
  unless that destination is a declared aperture it selected.
- A name outside what the ADR allows is refused the way the ADR states.
- Portable lane: the fact is absent and a start selecting a resolver aperture is refused by name
  before dispatch.
- The bytes that crossed the resolver aperture are counted and held to its ceiling.
- `cargo xtask check-bundles` through the new version and `cargo xtask check-json` pass in
  `scripts/gate.sh`; every earlier bundle is byte-unchanged.

## Boundaries

- No resolver for a run that did not select one.
- No change to the pinning of ordinary apertures: their hosts are still resolved once, at
  declaration.
- The dial outcome: `story:aperture-reports-dial-outcome`.

## Order

After `story:aperture-reports-dial-outcome`, as session `loom` asked: both change
`crates/substrate-host/src/egress.rs` and the same wire and gate files, and this story's bundle
is the later one. Its ADR takes the next free number in `adr/` when the story starts.
`story:confinement-profile-ingress-routes` is ordered after this story: it also adds an ADR and an
`adr/README.md` row and changes `crates/substrate-host/src/probe.rs` and
`crates/substrate-daemon/tests/runtime_vectors.rs`. Session `loom` set this story's priority low.

Not ordered against this story: the older drafts `story:host-process-adopt` and
`story:host-process-container-records`, which plan a successor bundle on the same wire and gate
files, and the proposed `story:docker-driver-entry-gate`, which also changes
`crates/substrate-daemon/tests/runtime_vectors.rs`. None of them is being worked; whichever
starts while this story is open is ordered then.

## Scope

- `adr` (a new ADR), `adr/README.md`. Inferred.
- `crates/substrate-host/src/egress.rs`. Cited: `:1204-1216` (the relay's dial).
- `crates/substrate-host/src/process.rs`, `crates/substrate-host/src/probe.rs`. Inferred.
- `crates/substrate-wire/src/lib.rs`, `crates/substrate-daemon/tests/runtime_vectors.rs`,
  `xtask/bundle-source`, `contracts/substrate-wire`, `xtask/src/bundle.rs`, `scripts/gate.sh`.
  Inferred.
- `AGENTS.md`, `README.md`, `CHANGELOG.md`. Inferred.
