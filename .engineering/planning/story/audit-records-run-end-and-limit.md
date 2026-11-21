---
format: aep.planning-md/3
id: story:audit-records-run-end-and-limit
kind: story
status: active
title: The audit sink receives a record when a run ends, naming the limit that ended it
summary: 'AuditRecord gains run_ended: one per started run, with the end state, the exit and the limit.'
owner: substrate
tags:
- confinement
- resource-bounds
- sdc
relations:
- decomposes: epic:runs-are-governed-and-limit-hits-are-events
- depends_on: story:run-observation-names-the-limit-that-ended-it
scope:
- confidence: inferred
  path: crates/substrate-host/src/confinement/linux.rs
- confidence: inferred
  path: crates/substrate-host/src/confinement/local.rs
- confidence: inferred
  path: crates/substrate-host/src/confinement/macos.rs
- confidence: cited
  path: crates/substrate-host/src/confinement/mod.rs
- confidence: inferred
  path: crates/substrate-host/src/confinement/oci.rs
- confidence: inferred
  path: crates/substrate-host/src/confinement/pod.rs
- confidence: inferred
  path: crates/substrate-host/src/confinement/systemd.rs
- confidence: cited
  path: crates/substrate-host/tests/confinement-public-items.txt
- confidence: inferred
  path: docs/design/36-limit-hits.md
- confidence: inferred
  path: spec/confinement-suite.json
- confidence: cited
  path: spec/confinement/confinement.yaml
- confidence: inferred
  path: spec/confinement/scenarios
- confidence: inferred
  path: website/docs
revision: 9
transitions:
- {from: "draft", to: "proposed", at: "2026-11-21T02:33:43Z", actor: "human:timo", revision: 8, decided_on: {"recorded":{"review_outcome":2}}, correlation: "run-governance-first-slice-accepted"}
- {from: "proposed", to: "active", at: "2026-11-21T02:33:43Z", actor: "human:timo", revision: 9, decided_on: {"recorded":{"review_outcome":2}}, correlation: "run-governance-first-slice-accepted"}
---
# Story: The audit sink receives a record when a run ends, naming the limit that ended it

## Outcome

A profile's `AuditSink` receives one `run_ended` record for every run that received `run_started`.
The adapter writes it when the run ends, whether or not anybody calls `observe` or `wait`. The
record carries the run, the profile, the adapter, the end state, the exit and the limit that ended
the run where `story:run-observation-names-the-limit-that-ended-it` observed one. An embedder that
only listens to the sink learns of every limit that ended a run without polling.

## Why

- `AuditRecord` has four variants: `ProfileAdmitted`, `ProfileRefused`, `RunStarted` and
  `PrivateTmpdirLeft` (`crates/substrate-host/src/confinement/mod.rs:280-324`). None records a
  run's end or a limit.
- The relayed requirement of 2026-11-21 wants "every limit hit visible as an event"
  (`epic:runs-are-governed-and-limit-hits-are-events`). The sink is the in-process path's only
  event channel (`crates/substrate-host/src/confinement/mod.rs:349-363`).
- The specification models only the admission record and leaves the others unmapped
  (`spec/confinement/confinement.yaml:300-304`, `:664`).
- The `linux` adapter already awaits a run's end in a task of its own for piped standard input
  (`crates/substrate-host/src/confinement/linux.rs:491-496`), so an adapter can act at the end
  without a caller.

## What changes

1. `docs/design/36-limit-hits.md` gains the record before code (`AGENTS.md` invariant 8). It
   decides:
   - the record's fields and its `event` name, `run_ended`;
   - which task calls the sink (`mod.rs:349-351` states that for today's records), and that a sink
     that blocks does not delay `wait`;
   - that it is written once per run, also for a run that ends `Unknown`, and what happens to a
     run still running when its adapter is dropped;
   - what a declared-insecure run's record carries.
2. `spec/confinement/confinement.yaml` states the record, or marks it `UNMAPPED` with the reason,
   as it does for refusal records today; `cargo xtask check-ess` regenerates the suite.
3. Every adapter writes the record when its run ends.

## Acceptance

A run on the `linux` adapter in the delegated lane that exceeds its memory bound, and that nobody
observes or waits on after `start_run` returned, puts exactly one `run_ended` record naming the
memory limit into its profile's sink after its `run_started` record; before this story the sink
receives nothing after `run_started`.

Checks:

- The design section is merged before any code of this story.
- A run that exits 0 writes one `run_ended` with no limit. Calling `observe` and `wait` on an
  ended run writes no second record.
- A profile without a sink writes nothing and runs as before.
- `local`, `macos`, `systemd`, `oci` and `pod` each have a case for one `run_ended` per run, with
  no `observe` or `wait` call, on the lane that can run it.
- The record serializes with `"event": "run_ended"`, and
  `crates/substrate-host/tests/confinement-public-items.txt` is regenerated in the same change.

## Boundaries

- No record for a denied execution or a denied connection (the epic's § Out of scope).
- No record while the run is still running.
- No process-count figure on the record: `story:process-count-limit-hit-is-observed` adds it.
- No wire event. The daemon's `exec.exited` and `exec.cancelled` events are unchanged
  (`crates/substrate-store/src/execs.rs:555-556`).

## Order

After `story:run-observation-names-the-limit-that-ended-it`: the record carries that story's
field.

## Scope

- `docs/design/36-limit-hits.md`. Inferred.
- `spec/confinement/confinement.yaml`. Cited: `:300-304`.
- `spec/confinement/scenarios`, `spec/confinement-suite.json`. Inferred.
- `crates/substrate-host/src/confinement/mod.rs`. Cited: `:280-324`, `:349-363`, `:507-530`.
- `crates/substrate-host/src/confinement/linux.rs`. Cited: `:491-496`.
- `local.rs`, `macos.rs`, `systemd.rs`, `oci.rs`, `pod.rs` in
  `crates/substrate-host/src/confinement`. Inferred: each adapter ends its own runs.
- `crates/substrate-host/tests/confinement-public-items.txt`. Cited: `:1-3`.
