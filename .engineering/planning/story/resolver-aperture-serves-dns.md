---
format: aep.planning-md/3
id: story:resolver-aperture-serves-dns
kind: story
status: draft
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
revision: 3
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
2. A successor contract bundle at the next free version, if the ADR adds a wire member
   (invariant 6).
3. The host driver generates the resolver configuration for a run that selects the aperture and
   serves the relay.

## Acceptance

In the delegated lane, a run that selects a declared resolver aperture resolves a name served by a
test resolver on loopback, and a run that selects none fails the same lookup; before this story no
run can resolve a name.

Checks:

- The ADR is accepted and indexed in `adr/README.md` before any code of this story.
- A run with the resolver aperture still cannot open a connection to an address it resolved
  unless that destination is a declared aperture it selected.
- A name outside what the ADR allows is refused the way the ADR states.
- Portable lane: the fact is absent and a start selecting a resolver aperture is refused by name
  before dispatch.
- The bytes that crossed the resolver aperture are counted and held to its ceiling.

## Boundaries

- No resolver for a run that did not select one.
- No change to the pinning of ordinary apertures: their hosts are still resolved once, at
  declaration.
- The dial outcome: `story:aperture-reports-dial-outcome`.

## Order

After `story:aperture-reports-dial-outcome`: both change
`crates/substrate-host/src/egress.rs` and, if this story cuts a bundle, the same wire and gate
files.

## Scope

- `adr` (a new ADR), `adr/README.md`. Inferred.
- `crates/substrate-host/src/egress.rs`. Cited: `:785`, `:1216`.
- `crates/substrate-host/src/process.rs`, `crates/substrate-host/src/probe.rs`. Inferred.
- `crates/substrate-wire/src/lib.rs`, `crates/substrate-daemon/tests/runtime_vectors.rs`,
  `xtask/bundle-source`, `contracts/substrate-wire`, `xtask/src/bundle.rs`, `scripts/gate.sh`.
  Inferred, and only if the ADR adds a wire member.
- `AGENTS.md`, `README.md`, `CHANGELOG.md`. Inferred.
