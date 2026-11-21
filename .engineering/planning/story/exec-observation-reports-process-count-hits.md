---
format: aep.planning-md/3
id: story:exec-observation-reports-process-count-hits
kind: story
status: draft
title: An exec's observation reports process-count hits without measurements
summary: 'A successor bundle after the executable-lists one: the ended exec observation carries the pids.events max count for every start, so a remote client reads what an embedder reads.'
owner: substrate
tags:
- confinement
- resource-bounds
- sdc
- wire
relations:
- decomposes: epic:runs-are-governed-and-limit-hits-are-events
- depends_on: story:process-count-limit-hit-is-observed
- depends_on: story:exec-start-carries-executable-lists
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
  path: crates/substrate-host/src/observation.rs
- confidence: cited
  path: crates/substrate-host/src/process.rs
- confidence: cited
  path: crates/substrate-wire/src/lib.rs
- confidence: inferred
  path: docs/design/36-limit-hits.md
- confidence: inferred
  path: scripts/gate.sh
- confidence: inferred
  path: xtask/bundle-source
- confidence: inferred
  path: xtask/src/bundle
- confidence: inferred
  path: xtask/src/bundle.rs
revision: 7
---
# Story: An exec's observation reports process-count hits without measurements

## Outcome

On the wire, an exec that reached its process-count bound says so on its ended observation, also
when the start did not ask for measurements. A client of a remote daemon reads the same count an
embedder reads on `RunObservation`.

## Why

- The host reads the count from `pids.events` `max` (`crates/substrate-host/src/process.rs:4181`)
  into `ResourceUsage.process_limit_hits` (`crates/substrate-wire/src/lib.rs:2174`). That usage
  exists only for a start that asked for measurements.
- `record_resource_bound` names memory, CPU and timeout and has no branch for the process count
  (`crates/substrate-host/src/process.rs:3280-3322`). A process-count hit does not end a run, so
  it is not a refusal on the observation either.
- The wire's closed event union has no limit transition
  (`crates/substrate-wire/src/lib.rs:2308-2318`).
- The relayed requirement of 2026-11-21 says runs move remote and every limit hit is visible
  (`epic:runs-are-governed-and-limit-hits-are-events`).

## What changes

1. `docs/design/36-limit-hits.md` gains a wire section before code (`AGENTS.md` invariant 8). It
   decides the optional member on the exec observation that carries the count, when it is present
   (an ended exec on a host whose snapshot publishes `exec.resource-usage` with
   `process_limit_hits`, `crates/substrate-wire/src/lib.rs:4355`), and consumer compatibility: an
   observation without the member serializes as on the predecessor bundle. It states that no event
   transition is added: the `exec.exited` and `exec.cancelled` events already carry the exec
   resource (`crates/substrate-store/src/execs.rs:553-571`), so the member reaches an event
   reader with it.
2. A successor bundle at the next free version when the story starts, after the one
   `story:exec-start-carries-executable-lists` cut, rendered with `cargo xtask render-bundle`, its
   check added to `scripts/gate.sh` (invariant 6).
3. The host driver copies the count into the wire member from its host-internal end-of-run member.
   `story:process-count-limit-hit-is-observed` owns the read of `pids.events` for every ended
   exec; this story adds no second read.
4. The daemon and the Rust SDK advertise the new bundle.

## Acceptance

In the delegated lane of `crates/substrate-daemon/tests/runtime_vectors.rs`, an exec started
without measurements with a process bound of 4 that tries to start 16 processes ends with an
observation whose process-count member is above zero; on the predecessor bundle that observation
has no such member.

Checks:

- The design section is merged before any code of this story.
- An exec that stays under its bound reports zero. Portable lane: no exec runs, and the vector for
  the member is absent, not passed.
- `ResourceUsage.process_limit_hits` for a start that asked for measurements is unchanged.
- `cargo xtask check-bundles` through the new version and `cargo xtask check-json` pass in
  `scripts/gate.sh`; every earlier bundle is byte-unchanged.
- `DELEGATED_CASES` and the counts `AGENTS.md` states are updated in the same change.

## Boundaries

- No new event transition and no refusal code for a process-count hit.
- The in-process field and record: `story:process-count-limit-hit-is-observed`.

## Order

After `story:process-count-limit-hit-is-observed`: it uses that story's design section and its
read of the counter. After `story:exec-start-carries-executable-lists` only because both cut a
successor bundle and change `crates/substrate-wire/src/lib.rs`, `scripts/gate.sh` and
`crates/substrate-daemon/tests/runtime_vectors.rs`; this story's bundle is the later one.

## Scope

- `docs/design/36-limit-hits.md`. Inferred.
- `crates/substrate-wire/src/lib.rs`. Cited: `:2174`, `:2308-2318`, `:4355`.
- `crates/substrate-host/src/process.rs`. Cited: `:3280-3322`, `:4181`.
- `xtask/bundle-source`, `contracts/substrate-wire` (one new version directory in each),
  `xtask/src/bundle.rs`, `xtask/src/bundle` (a new module), `scripts/gate.sh`. Inferred from
  `AGENTS.md` § The gate.
- `crates/substrate-host/src/observation.rs`. Inferred.
- `crates/substrate-daemon/tests/runtime_vectors.rs`. Inferred.
- `AGENTS.md`, `README.md`, `CHANGELOG.md`. Inferred.
