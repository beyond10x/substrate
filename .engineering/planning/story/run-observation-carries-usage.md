---
format: aep.planning-md/3
id: story:run-observation-carries-usage
kind: story
status: draft
title: A run's observation carries the usage behind its limits
summary: The ended RunObservation and the run_ended record carry wall time, CPU time, peak memory and peak process count on linux and systemd; absent elsewhere.
owner: substrate
tags:
- confinement
- resource-bounds
- sdc
relations:
- informed_by: epic:runs-are-governed-and-limit-hits-are-events
- depends_on: story:process-count-limit-hit-is-observed
scope:
- confidence: cited
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
- confidence: cited
  path: crates/substrate-host/src/confinement/systemd.rs
- confidence: cited
  path: crates/substrate-host/tests/confinement-public-items.txt
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
revision: 3
---
# Story: A run's observation carries the usage behind its limits

## Outcome

An ended `RunObservation` on the in-process path carries the run's resource usage: wall time, CPU
time, peak memory and peak process count. An embedder reads how close a run came to each bound,
and what a run that hit one had used. An adapter that does not measure a figure leaves it absent.

## Where the requirement comes from

Session `sdc` asked on 2026-11-21, for Loom's story `child-launches-through-confinement-adapters`:
the limit that ended a run "and the usage figures behind it", for the SDC KPI "limit hit" and its
budget and limit decisions. The limit itself is
`story:run-observation-names-the-limit-that-ended-it`; the process-count hits are
`story:process-count-limit-hit-is-observed`. Neither carries usage.

## Why

- `RunObservation` has no usage field (`crates/substrate-host/src/confinement/mod.rs:696-706`).
- The host driver measures usage only for a start that asks for it, and the `linux` adapter asks
  for none: it sends `measurements: BTreeSet::new()`
  (`crates/substrate-host/src/confinement/linux.rs:427`).
- The figures exist on the wire as `ResourceUsage`: `wall_time_us`, `cpu_time_us`,
  `memory_peak_bytes`, `processes_peak` (`crates/substrate-wire/src/lib.rs:2161-2185`). A daemon
  client can read them today; an embedder cannot.
- `systemd` already reads `cpu.stat` for its CPU budget
  (`crates/substrate-host/src/confinement/systemd.rs:837-840`).

## What changes

1. `docs/design/36-limit-hits.md` gains a usage section before code (`AGENTS.md` invariant 8). It
   decides the field and its members, whether the profile asks for usage or every run with
   `process_bounds` gets it, what measuring costs a run, and one row per adapter: `linux` from the
   host driver's usage, `systemd` from its run cgroup, `oci` and `pod` from what their runtime
   states or absent, `local` and `macos` absent.
2. `spec/confinement/confinement.yaml` states the field and a scenario;
   `cargo xtask check-ess` regenerates the suite.
3. The adapters report the figures as their design row says, and the `run_ended` record of
   `story:audit-records-run-end-and-limit` carries the same figures.

## Acceptance

In the delegated lane, a run on the `linux` adapter that allocates 64 MiB and burns one second of
CPU ends with an observation whose peak memory is at least 64 MiB and whose CPU time is at least
one second; before this story `RunObservation` has no usage.

Checks:

- The design section is merged before any code of this story.
- The same case passes on the `systemd` lane for `systemd` around `local`.
- `local` and `macos` leave the field absent (invariant 3).
- A run that a limit ended carries both the limit and its usage, on the observation and on its
  `run_ended` record.
- No file under `contracts/` or `crates/substrate-wire` changes.
- `crates/substrate-host/tests/confinement-public-items.txt` is regenerated in the same change.

## Boundaries

- No usage while the run is still running.
- No block-IO or network figures (`story:block-io-and-bandwidth-limits`).
- No wire change: the wire already carries `ResourceUsage`.

## Order

Informed by `epic:runs-are-governed-and-limit-hits-are-events` and not part of it. After
`story:process-count-limit-hit-is-observed`: it adds members to the same `RunObservation`,
`run_ended` record and design document, and changes the same files.

## Scope

- `docs/design/36-limit-hits.md`. Inferred.
- `spec/confinement/confinement.yaml`, `spec/confinement/scenarios`, `spec/confinement-suite.json`.
  Inferred.
- `crates/substrate-host/src/confinement/mod.rs`. Cited: `:696-706`.
- `crates/substrate-host/src/confinement/linux.rs`. Cited: `:427`, `:507-526`.
- `crates/substrate-host/src/confinement/systemd.rs`. Cited: `:837-840`.
- `crates/substrate-host/src/confinement/oci.rs`, `pod.rs`, `local.rs`, `macos.rs` in the same
  directory. Inferred: the struct literal gains a field.
- `crates/substrate-host/tests/confinement-public-items.txt`. Cited: `:1-3`.
- `website/docs`. Inferred.
