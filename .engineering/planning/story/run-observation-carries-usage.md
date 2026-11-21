---
format: aep.planning-md/3
id: story:run-observation-carries-usage
kind: story
status: active
title: A run's observation carries the usage behind its limits
summary: The ended RunObservation and the run_ended record carry wall time, CPU time, peak memory and peak process count on linux and systemd; absent on every other adapter.
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
  path: crates/substrate-host/src/observation.rs
- confidence: cited
  path: crates/substrate-host/src/process.rs
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
revision: 9
transitions:
- {from: "draft", to: "proposed", at: "2026-11-21T03:08:31Z", actor: "human:timo", revision: 8, decided_on: {"recorded":{"review_outcome":3}}, correlation: "sdc-accepts-usage-and-file-changes-2026-11-21"}
- {from: "proposed", to: "active", at: "2026-11-21T03:08:31Z", actor: "human:timo", revision: 9, decided_on: {"recorded":{"review_outcome":3}}, correlation: "sdc-accepts-usage-and-file-changes-2026-11-21"}
---
# Story: A run's observation carries the usage behind its limits

## Outcome

An ended `RunObservation` on the in-process path carries the run's resource usage: wall time, CPU
time, peak memory and peak process count. An embedder reads how close a run came to each bound,
and what a run that hit one had used. `linux` and `systemd` report the four figures; every other
adapter leaves the field absent.

## Where the requirement comes from

Session `sdc` asked on 2026-11-21, for Loom's story `child-launches-through-confinement-adapters`:
the limit that ended a run "and the usage figures behind it", for the SDC KPI "limit hit" and its
budget and limit decisions. It accepted this story the same day under
`approval-record:sdc-decides-plan-and-design-2026-11-21` and ordered it directly after
`story:process-count-limit-hit-is-observed`. The limit itself is
`story:run-observation-names-the-limit-that-ended-it`; the process-count hits are
`story:process-count-limit-hit-is-observed`. Neither carries usage.

## Why

- `RunObservation` has no usage field (`crates/substrate-host/src/confinement/mod.rs:696-706`).
- The host driver measures usage only for a start that asks for it
  (`crates/substrate-host/src/process.rs:3115-3122`), and the `linux` adapter asks for none: it
  sends `measurements: BTreeSet::new()` (`crates/substrate-host/src/confinement/linux.rs:427`).
- The figures exist on the wire as `ResourceUsage`: `wall_time_us`, `cpu_time_us`,
  `memory_peak_bytes`, `processes_peak` (`crates/substrate-wire/src/lib.rs:2161-2185`). A daemon
  client can read them today; an embedder cannot.
- `systemd` around `local` owns its run's cgroup
  (`crates/substrate-host/src/confinement/systemd.rs:938-946`). It samples `cpu.stat` for its CPU
  budget (`crates/substrate-host/src/confinement/systemd.rs:836-842`) and reads no `memory.peak`,
  no `pids.peak` and no wall time today.

## What changes

1. `docs/design/36-limit-hits.md` gains a usage section before code (`AGENTS.md` invariant 8). It
   decides the field and its members and the `systemd` row, and it records the two decisions
   below.
2. **One carrier.** The four figures extend the host-internal end-of-run member that
   `story:run-observation-names-the-limit-that-ended-it` adds to `ExecObservation`
   (`crates/substrate-host/src/observation.rs:19-34`) and
   `story:process-count-limit-hit-is-observed` extends. The host driver reads them when every exec
   ends, before `reconcile_cgroup` removes the cgroup. The `linux` adapter does not ask for the
   wire's `measurements`: a measured start that cannot read a counter is refused
   `exec.metrics-unavailable` (`crates/substrate-host/src/process.rs:3290-3295`), and an embedder
   that asked for nothing must not get that refusal. A figure the host could not read is absent.
   The wire's `resource.usage` is unchanged and nothing replaces it.
3. **`systemd` reads its own cgroup.** A run of `systemd` around `local` never passes through the
   host driver. When such a run ends and before its cgroup is removed, the adapter reads
   `cpu.stat`, `memory.peak` and `pids.peak` from the run cgroup it created and takes the wall
   time from its own start instant. `systemd` around `linux` reports the host driver's figures.
4. **`oci`, `pod`, `local` and `macos` leave the field absent** in this story.
5. `spec/confinement/confinement.yaml` states the field and a scenario;
   `cargo xtask check-ess` regenerates the suite.
6. The `run_ended` record of `story:audit-records-run-end-and-limit` carries the same figures.

## Acceptance

In the delegated lane, a run on the `linux` adapter that sleeps one second, allocates 64 MiB,
burns one second of CPU and starts 8 processes ends with an observation whose wall time is at
least one second, whose CPU time is at least one second, whose peak memory is at least 64 MiB and
whose peak process count is at least 8; before this story `RunObservation` has no usage.

Checks:

- The design section is merged before any code of this story.
- The same case passes on the `systemd` lane for `systemd` around `local`.
- `local` and `macos` leave the field absent, and so do `oci` and `pod` on their lanes
  (invariant 3).
- A run that a limit ended carries both the limit and its usage, on the observation and on its
  `run_ended` record.
- A start through the daemon without `measurements` still has no `resource.usage`, and no file
  under `contracts/` or `crates/substrate-wire` changes.
- `crates/substrate-host/tests/confinement-public-items.txt` is regenerated in the same change.

## Boundaries

- No usage while the run is still running.
- No usage on `oci` or `pod`.
- No block-IO or network figures (`story:block-io-and-bandwidth-limits`).
- No wire change: the wire already carries `ResourceUsage`.

## Order

Informed by `epic:runs-are-governed-and-limit-hits-are-events` and not part of it. After
`story:process-count-limit-hit-is-observed`: it adds members to the same `RunObservation`,
host-internal member, `run_ended` record and design document. Every later draft of that epic is
ordered after it through `story:exec-file-changes-are-recorded`, which follows this story; the
shared files are `spec/confinement/confinement.yaml`, `crates/substrate-host/src/confinement`,
`crates/substrate-host/src/process.rs` and `docs/design/36-limit-hits.md`.

## Scope

- `docs/design/36-limit-hits.md`. Inferred.
- `spec/confinement/confinement.yaml`, `spec/confinement/scenarios`, `spec/confinement-suite.json`.
  Inferred.
- `crates/substrate-host/src/confinement/mod.rs`. Cited: `:696-706`.
- `crates/substrate-host/src/confinement/linux.rs`. Cited: `:427`, `:507-526`.
- `crates/substrate-host/src/confinement/systemd.rs`. Cited: `:836-842`, `:938-946`.
- `crates/substrate-host/src/process.rs`. Cited: `:3115-3128`, `:3290-3295`.
- `crates/substrate-host/src/observation.rs`. Cited: `:19-34`.
- `crates/substrate-host/src/confinement/oci.rs`, `pod.rs`, `local.rs`, `macos.rs` in the same
  directory. Inferred: the struct literal gains a field.
- `crates/substrate-host/tests/confinement-public-items.txt`. Cited: `:1-3`.
- `website/docs`. Inferred.
