---
format: aep.planning-md/3
id: story:process-count-limit-hit-is-observed
kind: story
status: active
title: A process-count limit hit is observed and recorded in process
summary: pids.events max reaches RunObservation and the run_ended audit record on linux and systemd; the wire is story:exec-observation-reports-process-count-hits.
owner: substrate
tags:
- confinement
- resource-bounds
- sdc
relations:
- decomposes: epic:runs-are-governed-and-limit-hits-are-events
- depends_on: story:audit-records-run-end-and-limit
scope:
- confidence: inferred
  path: crates/substrate-host/src/confinement/linux.rs
- confidence: cited
  path: crates/substrate-host/src/confinement/mod.rs
- confidence: cited
  path: crates/substrate-host/src/confinement/systemd.rs
- confidence: cited
  path: crates/substrate-host/src/observation.rs
- confidence: cited
  path: crates/substrate-host/src/process.rs
- confidence: cited
  path: crates/substrate-host/tests/confinement-public-items.txt
- confidence: cited
  path: crates/substrate-host/tests/consumer_surface.rs
- confidence: inferred
  path: docs/design/36-limit-hits.md
- confidence: inferred
  path: spec/confinement-suite.json
- confidence: inferred
  path: spec/confinement/confinement.yaml
- confidence: inferred
  path: spec/confinement/scenarios
- confidence: inferred
  path: website/docs
revision: 13
transitions:
- {from: "draft", to: "proposed", at: "2026-11-21T02:33:43Z", actor: "human:timo", revision: 12, decided_on: {"recorded":{"review_outcome":3}}, correlation: "run-governance-first-slice-accepted"}
- {from: "proposed", to: "active", at: "2026-11-21T02:33:43Z", actor: "human:timo", revision: 13, decided_on: {"recorded":{"review_outcome":3}}, correlation: "run-governance-first-slice-accepted"}
---
# Story: A process-count limit hit is observed and recorded in process

## Outcome

A run on the in-process path that reached its process-count bound is visible as such. Its ended
`RunObservation` carries the number of refused process creations, and its `run_ended` audit record
carries the same number. An adapter that does not read the counter leaves both absent.

## Why

- A process-count hit does not end a run: the kernel refuses the `fork` and the program carries
  on or fails in its own way. So no limit names it today.
- The host reads the count from `pids.events` `max` (`crates/substrate-host/src/process.rs:4181`)
  into `ResourceUsage.process_limit_hits` (`crates/substrate-wire/src/lib.rs:2174`). That usage
  exists only for a start that asked for measurements.
- `RunObservation` has no field for it (`crates/substrate-host/src/confinement/mod.rs:696-706`),
  and `AuditRecord` has no variant that could carry it
  (`crates/substrate-host/src/confinement/mod.rs:280-324`).
- `systemd` writes `pids.max` on a cgroup it creates
  (`crates/substrate-host/src/confinement/systemd.rs:938-946`), so it can read that cgroup's
  `pids.events`.

## What changes

1. `docs/design/36-limit-hits.md` gains the process count before code (`AGENTS.md` invariant 8).
   It decides:
   - the `RunObservation` field: a count, absent where the adapter does not read it;
   - the same count on the `run_ended` record of `story:audit-records-run-end-and-limit`;
   - whether an observation of a run that is still running carries the count, and what a consumer
     that needs it before the end does (it polls `observe`);
   - that the host driver reads `pids.events` `max` when every exec ends, measured or not, into
     the host-internal end-of-run member `story:run-observation-names-the-limit-that-ended-it`
     added to `ExecObservation` (`crates/substrate-host/src/observation.rs:19-34`). This story
     owns that read; `story:exec-observation-reports-process-count-hits` only publishes the value
     on the wire;
   - one row per adapter: `linux` and `systemd` read `pids.events`; `oci` and `pod` state what
     their runtime exposes or report the field absent.
2. `spec/confinement/confinement.yaml` states the field and a scenario;
   `cargo xtask check-ess` regenerates the suite.
3. `linux` and `systemd` report the count; the other adapters as their design row says.

## Acceptance

In the delegated lane, a run on the `linux` adapter with a process bound of 4 that tries to start
16 processes ends with an observation whose process-count hits are above zero, and its sink holds
one `run_ended` record with the same count; before this story neither carries a count.

Checks:

- The design section is merged before any code of this story.
- A run on `linux` that stays under its bound reports zero hits on both.
- The same two cases pass on the `systemd` lane for `systemd` around `local`.
- `local` and `macos` leave the field absent on both (invariant 3).
- No file under `contracts/` or `crates/substrate-wire` changes.
- `crates/substrate-host/tests/consumer_surface.rs` gains one delegated case, written against the
  public items only: one profile on the `linux` adapter names an allow-list, a deny-list and
  process bounds; the run sees a denied execution fail, exceeds its process bound and then its
  timeout; its observation and its `run_ended` record each name the wall-time limit and a
  process-count above zero. This is the test the epic's § Done when names.
- `crates/substrate-host/tests/confinement-public-items.txt` is regenerated in the same change.

## Boundaries

- A process-count hit never ends the run and never becomes a limit of
  `story:run-observation-names-the-limit-that-ended-it`.
- No record at the moment of the hit. The count reaches the sink on `run_ended`.
- The wire: `story:exec-observation-reports-process-count-hits`.

## Order

After `story:audit-records-run-end-and-limit`: it adds a field to that story's record.

## Scope

- `docs/design/36-limit-hits.md`. Inferred.
- `spec/confinement/confinement.yaml`, `spec/confinement/scenarios`, `spec/confinement-suite.json`.
  Inferred.
- `crates/substrate-host/src/confinement/mod.rs`. Cited: `:280-324`, `:696-706`.
- `crates/substrate-host/src/confinement/systemd.rs`. Cited: `:938-946`.
- `crates/substrate-host/src/confinement/linux.rs`. Inferred.
- `crates/substrate-host/src/process.rs`. Cited: `:4181`.
- `crates/substrate-host/src/observation.rs`. Cited: `:19-34`.
- `crates/substrate-host/tests/consumer_surface.rs`. Cited: `:110`, `:362`.
- `website/docs`. Inferred: `cargo xtask check-docs` holds the site's contract tables to the spec.
- `crates/substrate-host/tests/confinement-public-items.txt`. Cited: `:1-3`.
