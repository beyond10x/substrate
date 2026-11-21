---
format: aep.planning-md/3
id: story:aperture-reports-dial-outcome
kind: story
status: active
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
  path: crates/substrate-host/src/confinement/mod.rs
- confidence: cited
  path: crates/substrate-host/src/egress.rs
- confidence: inferred
  path: crates/substrate-host/src/process.rs
- confidence: inferred
  path: crates/substrate-host/tests/confinement-public-items.txt
- confidence: cited
  path: crates/substrate-wire/src/lib.rs
- confidence: inferred
  path: docs/design/38-aperture-dial-outcome.md
- confidence: inferred
  path: scripts/gate.sh
- confidence: inferred
  path: spec/confinement
- confidence: inferred
  path: xtask/bundle-source
- confidence: inferred
  path: xtask/src/bundle.rs
revision: 11
transitions:
- {from: "draft", to: "proposed", at: "2026-11-21T03:55:19Z", actor: "human:timo", revision: 10, decided_on: {"recorded":{"review_outcome":5}}, executor: "agent:substrate", correlation: "loom-accepts-aperture-stories-2026-11-21"}
- {from: "proposed", to: "active", at: "2026-11-21T03:55:19Z", actor: "human:timo", revision: 11, decided_on: {"recorded":{"review_outcome":5}}, executor: "agent:substrate", correlation: "loom-accepts-aperture-stories-2026-11-21"}
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
- The forwarder dials the pinned address itself, in `relay_body`
  (`crates/substrate-host/src/egress.rs:1204-1216`); the child only ever connects to loopback
  inside its namespace
  (`README.md` § Egress apertures). So the child cannot distinguish a refused destination from a
  closed relay, and the caller sees `bytes` of zero in both cases.
- That dial is a blocking `connect` with no deadline of its own
  (`crates/substrate-host/src/egress.rs:1216`). A destination that never answers is given up on
  only when the kernel gives up or the run ends.
- Not established here: what the child's socket sees when the forwarder's dial fails. The design
  document records it from a probe.
- Session `loom` reads the counts after one dial of one aperture per launch: one connected, one
  refused or one timed out.

## What changes

1. **A design document first** (`AGENTS.md` invariant 8),
   `docs/design/38-aperture-dial-outcome.md`. It decides the counters and their names and **a
   dial deadline, which is new behaviour**: the forwarder gives up a dial that has not connected
   within it and counts it as timed out. Today such a dial waits for the kernel. The document
   sets the deadline's value, says whether the operator can change it per aperture, and states
   that a destination slower than the deadline, which connects today, no longer does. It also
   decides whether the last error's class is
   reported, the bound on what is kept, how the counts cross from the forwarder process to the
   driver (the byte counters already cross; the document says how), and consumer compatibility:
   a record without the new member serializes as on the predecessor bundle.
2. A successor contract bundle at the next free version adds the optional member to the applied
   network record, rendered with `cargo xtask render-bundle`, its check added to `scripts/gate.sh`
   (invariant 6).
3. The forwarder counts each dial's outcome and the host driver reports the counts on the exec's
   applied record.
4. The embedder route. `RunObservation` has no network member today
   (`crates/substrate-host/src/confinement/mod.rs:696-706`), so an embedder on the adapter route
   cannot read an applied network record at all. The design document decides how it reads the
   counts: a member on `RunObservation`, or a call on the adapter. That member or call is this
   story's; the limit, usage and process-count members are the run-observation stories', which
   this story follows.

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
- A run on the in-process `linux` adapter with the same refused dial exposes the same counts to
  the embedder, and `crates/substrate-host/tests/confinement-public-items.txt` is regenerated in
  the same change.
- `cargo xtask check-bundles` through the new version and `cargo xtask check-json` pass in
  `scripts/gate.sh`; every earlier bundle is byte-unchanged.

## Boundaries

- No event per dial and no record while the run is still running.
- No change to which destinations a run may name or select. The one change in what it reaches is
  the dial deadline above.
- No DNS: `story:resolver-aperture-serves-dns`.

## Order

It cuts a successor bundle and changes `crates/substrate-wire/src/lib.rs`, `scripts/gate.sh` and
`crates/substrate-daemon/tests/runtime_vectors.rs`, which
`story:exec-file-change-record-on-the-wire` and the wire stories before it also change. It is
worked after that story, and it takes the next free bundle version when it starts. Through that
edge it is also after the limit, usage and process-count stories, which add the other members of
`RunObservation` in `crates/substrate-host/src/confinement/mod.rs`.
`story:resolver-aperture-serves-dns` follows it, then
`story:confinement-profile-ingress-routes` and
`story:explicitly-empty-exec-subpaths-means-the-program-only`, which share
`crates/substrate-daemon/tests/runtime_vectors.rs` or
`crates/substrate-host/src/confinement/linux.rs` with it. Session `loom` set its priority low:
it takes no capacity from the wire stories before it.

Not ordered against this story: the older drafts `story:host-process-adopt` and
`story:host-process-container-records` (each plans a successor bundle and still names `0.23.0`)
and the proposed `story:docker-driver-entry-gate`
(`crates/substrate-daemon/tests/runtime_vectors.rs`). None of them is being worked. Whichever of
them starts while this story is open is ordered then.

## Scope

- `docs/design/38-aperture-dial-outcome.md` (new). Inferred.
- `crates/substrate-wire/src/lib.rs`. Cited: `:1874-1890`.
- `crates/substrate-host/src/egress.rs`. Cited: `:1204-1216`.
- `crates/substrate-host/src/process.rs`, `crates/substrate-host/src/confinement/linux.rs`.
  Inferred.
- `crates/substrate-host/src/confinement/mod.rs`. Cited: `:696-706`.
- `crates/substrate-host/tests/confinement-public-items.txt`, `spec/confinement`. Inferred: the
  embedder route.
- `crates/substrate-daemon/tests/runtime_vectors.rs`, `xtask/bundle-source`,
  `contracts/substrate-wire`, `xtask/src/bundle.rs`, `scripts/gate.sh`. Inferred.
- `AGENTS.md`, `README.md`, `CHANGELOG.md`. Inferred.
