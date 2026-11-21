---
format: aep.planning-md/3
id: story:exec-file-change-record-on-the-wire
kind: story
status: active
title: An exec's file-change record is readable on the wire and announced by an event
summary: 'A successor bundle: the fact, the start member, the routes for the record and its content, the event and the refusals, over the record story:exec-file-changes-are-recorded produces.'
owner: substrate
tags:
- observation
- sdc
- wire
relations:
- informed_by: epic:runs-are-governed-and-limit-hits-are-events
- depends_on: story:exec-file-changes-are-recorded
- depends_on: story:exec-observation-reports-process-count-hits
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
  path: crates/substrate-daemon/src/app
- confidence: inferred
  path: crates/substrate-daemon/tests/runtime_vectors.rs
- confidence: inferred
  path: crates/substrate-host/src/probe.rs
- confidence: inferred
  path: crates/substrate-kubernetes
- confidence: cited
  path: crates/substrate-store/src
- confidence: cited
  path: crates/substrate-wire/src/lib.rs
- confidence: inferred
  path: scripts/gate.sh
- confidence: inferred
  path: website/docs/reference/contract.md
- confidence: inferred
  path: xtask/bundle-source
- confidence: inferred
  path: xtask/src/bundle
- confidence: inferred
  path: xtask/src/bundle.rs
revision: 8
transitions:
- {from: "draft", to: "proposed", at: "2026-11-21T03:15:49Z", actor: "human:timo", revision: 7, decided_on: {"recorded":{"review_outcome":2}}, executor: "agent:substrate", correlation: "operator-standing-acceptance-2026-11-21"}
- {from: "proposed", to: "active", at: "2026-11-21T03:15:49Z", actor: "human:timo", revision: 8, decided_on: {"recorded":{"review_outcome":2}}, executor: "agent:substrate", correlation: "operator-standing-acceptance-2026-11-21"}
---
# Story: An exec's file-change record is readable on the wire and announced by an event

## Outcome

A client of the daemon reads an exec's file-change record and the content behind its digests
through a route, and receives an event when the record is complete. A remote client sees the same
record an embedder reads through the adapter API after
`story:exec-file-changes-are-recorded`.

## Where the requirement comes from

Session `sdc`, 2026-11-21, relaying Timo's requirement: the record is "visible as an event on the
wire and in the in-process audit path". Split from `story:exec-file-changes-are-recorded` so that
story's acceptance names one surface.

## Why

- The exec event transitions carry no file change (`crates/substrate-wire/src/lib.rs:2308-2318`).
- No route reads a per-exec change record. `GitChangeSet` is per Git workspace
  (`crates/substrate-wire/src/lib.rs:1502-1515`, `Workspace::git_changes` at
  `crates/b10x-substrate-sdk/src/lib.rs:1108`).
- The events a workspace has for file changes cover only Substrate's own file routes
  (`crates/substrate-store/src/operations.rs:854-855`).

## What changes

1. The ADR of `story:exec-file-changes-are-recorded` already states the wire shape. This story
   adds nothing to it; a change to that shape is an amendment to that ADR before code
   (`AGENTS.md` invariant 8).
2. A successor contract bundle at the next free version when the story starts, after the one
   `story:exec-observation-reports-process-count-hits` cut: the capability fact, the start member
   that asks for the record, the route that reads an exec's record, the route or range that reads
   content by digest, the event transition, and each named refusal. Rendered with
   `cargo xtask render-bundle`, its check added to `scripts/gate.sh` (invariant 6).
3. The daemon serves the routes by reading the host-driver call
   `story:exec-file-changes-are-recorded` added, keyed by the exec's id. It keeps no second copy
   of the record or its content; retention is the host driver's. It appends the event when the
   host driver reports the record complete.
4. The daemon and the Rust SDK advertise the new bundle.

## Acceptance

In the delegated lane of `crates/substrate-daemon/tests/runtime_vectors.rs`, the route returns the
change record of an ended exec that created, modified, deleted and renamed files, with the before
and after digests; on the predecessor bundle the route does not exist.

Checks:

- The event for that record arrives on the event stream after the exec's terminal event.
- The content route returns both sides of the modified file under the declared bound.
- Two execs that write one workspace at the same time: the route answers each with the named
  reason for an unavailable record, and no event announces a record.
- An exec that did not ask for the record: the route answers its named refusal.
- Portable lane: the fact is absent, a start asking for the record is refused by name before
  dispatch, and the vector for the record is absent, not passed.
- A unit test on the Kubernetes namespace driver's snapshot holds the fact absent.
- `cargo xtask check-bundles` through the new version and `cargo xtask check-json` pass in
  `scripts/gate.sh`; every earlier bundle is byte-unchanged.
- `PORTABLE_CASES`, `DELEGATED_CASES` and the counts `AGENTS.md` states are updated in the same
  change.

## Boundaries

- No Rust SDK method and no CLI command for the record; each is its own story once the bundle
  exists.
- No diff rendering. The client computes a diff from content.
- The record's fields, bounds and overlap rule are `story:exec-file-changes-are-recorded`'s.

## Order

After `story:exec-file-changes-are-recorded`: it publishes that story's record. After
`story:exec-observation-reports-process-count-hits` only because both cut a successor bundle and
change `crates/substrate-wire/src/lib.rs`, `scripts/gate.sh` and
`crates/substrate-daemon/tests/runtime_vectors.rs`; this story's bundle is the later one.
`story:confinement-profile-ingress-routes` is ordered after this story because both change
`crates/substrate-daemon/tests/runtime_vectors.rs`, `crates/substrate-host/src/probe.rs` and
`AGENTS.md`.

## Scope

- `crates/substrate-wire/src/lib.rs`. Cited: `:1502-1515`, `:2308-2318`.
- `crates/substrate-store/src`. Cited: `operations.rs:854-855`.
- `crates/substrate-daemon/src/app`, `crates/substrate-daemon/tests/runtime_vectors.rs`,
  `crates/substrate-host/src/probe.rs`, `crates/substrate-kubernetes`. Inferred.
- `xtask/bundle-source`, `contracts/substrate-wire` (one new version directory in each),
  `xtask/src/bundle.rs`, `xtask/src/bundle` (a new module), `scripts/gate.sh`. Inferred from
  `AGENTS.md` § The gate.
- `AGENTS.md`, `README.md`, `CHANGELOG.md`, `website/docs/reference/contract.md`. Inferred.
