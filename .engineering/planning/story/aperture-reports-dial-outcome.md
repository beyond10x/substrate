---
format: aep.planning-md/3
id: story:aperture-reports-dial-outcome
kind: story
status: draft
title: An aperture reports the outcome of each dial
summary: The applied network record counts connected, refused and timed-out dials per aperture, so a caller tells a dead destination from a child that never tried. Design document first; a successor bundle.
owner: substrate
tags:
- loom
- network
- wire
relations:
- depends_on: story:exec-file-change-record-on-the-wire
scope:
- confidence: inferred
  path: AGENTS.md
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: README.md
- confidence: inferred
  path: contracts/substrate-wire
- confidence: inferred
  path: crates/substrate-daemon/tests/runtime_vectors.rs
- confidence: inferred
  path: crates/substrate-host/src/confinement/linux.rs
- confidence: cited
  path: crates/substrate-host/src/egress.rs
- confidence: inferred
  path: crates/substrate-host/src/process.rs
- confidence: cited
  path: crates/substrate-wire/src/lib.rs
- confidence: inferred
  path: docs/design
- confidence: inferred
  path: scripts/gate.sh
- confidence: inferred
  path: xtask/bundle-source
- confidence: inferred
  path: xtask/src/bundle.rs
revision: 3
---
# Story: An aperture reports the outcome of each dial

## Outcome

For a run that used an egress aperture, the applied network record states what became of the
forwarder's dials to the pinned destination: how many connected, how many were refused and how
many timed out. A caller can tell "the destination was down" from "the child never tried" without
reading the child's output.

## Where the requirement comes from

Session `loom`, 2026-11-21: its `upstream-blocker:substrate-aperture-dial-outcome` holds Loom's
TCP-reachability story until an aperture reports the outcome of a dial (connected, refused, timed
out) to the caller. Session `loom` is the stakeholder of this story
(`AGENTS.md` § Planning artifacts).

## Why

Verified at `main` `638a7d70c`:

- `AppliedAperture` carries `mode`, `name`, `destination`, `mechanism`, `bytes` and `max_bytes`
  (`crates/substrate-wire/src/lib.rs:1874-1890`). No member states whether a dial succeeded.
- The forwarder dials the pinned address itself (`crates/substrate-host/src/egress.rs:785`,
  `:1216`); the child only ever connects to loopback inside its namespace
  (`README.md` § Egress apertures). So the child cannot distinguish a refused destination from a
  closed relay, and the caller sees `bytes` of zero in both cases.
- Not established here: what the child's socket sees when the forwarder's dial fails. The design
  document records it from a probe.

## What changes

1. **A design document first** (`AGENTS.md` invariant 8). It decides the counters and their
   names, what counts as a timeout and which deadline applies, whether the last error's class is
   reported, the bound on what is kept, how the counts cross from the forwarder process to the
   driver (the byte counters already cross; the document says how), and consumer compatibility:
   a record without the new member serializes as on the predecessor bundle.
2. A successor contract bundle at the next free version adds the optional member to the applied
   network record, rendered with `cargo xtask render-bundle`, its check added to `scripts/gate.sh`
   (invariant 6).
3. The forwarder counts each dial's outcome and the host driver reports the counts on the exec's
   applied record, and through the `linux` adapter to an embedder.

## Acceptance

In the delegated lane of `crates/substrate-daemon/tests/runtime_vectors.rs`, an exec whose child
connects once through an aperture declared to a loopback port nobody listens on ends with an
applied network record that counts one refused dial and no connected one; on the predecessor
bundle the record has no such member.

Checks:

- The design document is merged before any code of this story.
- An exec whose child connects once to a listening destination counts one connected dial.
- An exec whose child never connects counts zero of each.
- A destination that accepts no connection within the deadline counts one timed-out dial.
- With an aperture list, each element of the applied array carries its own counts.
- `cargo xtask check-bundles` through the new version and `cargo xtask check-json` pass in
  `scripts/gate.sh`; every earlier bundle is byte-unchanged.

## Boundaries

- No event per dial and no record while the run is still running.
- No change to which destinations a run may reach.
- No DNS: `story:resolver-aperture-serves-dns`.

## Order

It cuts a successor bundle and changes `crates/substrate-wire/src/lib.rs`, `scripts/gate.sh` and
`crates/substrate-daemon/tests/runtime_vectors.rs`, which
`story:exec-file-change-record-on-the-wire` and the wire stories before it also change. It is
worked after that story.

## Scope

- `docs/design` (a new document). Inferred.
- `crates/substrate-wire/src/lib.rs`. Cited: `:1874-1890`.
- `crates/substrate-host/src/egress.rs`. Cited: `:785`, `:1216`.
- `crates/substrate-host/src/process.rs`, `crates/substrate-host/src/confinement/linux.rs`.
  Inferred.
- `crates/substrate-daemon/tests/runtime_vectors.rs`, `xtask/bundle-source`,
  `contracts/substrate-wire`, `xtask/src/bundle.rs`, `scripts/gate.sh`. Inferred.
- `AGENTS.md`, `README.md`, `CHANGELOG.md`. Inferred.
