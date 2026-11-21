---
format: aep.planning-md/3
id: story:run-observation-names-the-limit-that-ended-it
kind: story
status: active
title: A run's observation names the limit that ended it
summary: RunObservation states the memory, CPU or wall-time limit that ended a run; memory_oom_kills on linux and systemd.
owner: substrate
tags:
- confinement
- resource-bounds
- sdc
relations:
- decomposes: epic:runs-are-governed-and-limit-hits-are-events
- depends_on: story:executable-deny-list
scope:
- confidence: cited
  path: crates/substrate-host/src/confinement/linux.rs
- confidence: cited
  path: crates/substrate-host/src/confinement/local.rs
- confidence: cited
  path: crates/substrate-host/src/confinement/macos.rs
- confidence: cited
  path: crates/substrate-host/src/confinement/mod.rs
- confidence: cited
  path: crates/substrate-host/src/confinement/oci.rs
- confidence: cited
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
revision: 11
transitions:
- {from: "draft", to: "proposed", at: "2026-11-21T02:33:42Z", actor: "human:timo", revision: 10, decided_on: {"recorded":{"review_outcome":5}}, correlation: "run-governance-first-slice-accepted"}
- {from: "proposed", to: "active", at: "2026-11-21T02:33:43Z", actor: "human:timo", revision: 11, decided_on: {"recorded":{"review_outcome":5}}, correlation: "run-governance-first-slice-accepted"}
---
# Story: A run's observation names the limit that ended it

## Outcome

`RunObservation` states which declared limit ended a run: memory, CPU time or wall time. It is
absent for a run no limit ended and absent where the adapter cannot observe it, never a guess.
`linux` and `systemd` also report `memory_oom_kills`, which they can read from the run's cgroup.

## Why

- `RunObservation` has no field for a limit
  (`crates/substrate-host/src/confinement/mod.rs:696-706`). An embedder sees an exit code or a
  signal and cannot tell a timeout from a crash.
- The host driver already knows. It records `exec.memory-limit`, `exec.cpu-limit` and
  `exec.timeout` on the exec observation (`crates/substrate-host/src/process.rs:3280-3322`). The
  `linux` adapter drops that when it builds a `RunObservation` and writes `memory_oom_kills: None`
  (`crates/substrate-host/src/confinement/linux.rs:507-526`).
- The host driver ends a run for two more bounds and names each on the observation:
  `exec.aperture-byte-limit` (`crates/substrate-host/src/process.rs:3402`) and the session output
  backpressure (`crates/substrate-host/src/process.rs:3324-3331`).
- `oci` reads the runtime's `OOMKilled` flag (`crates/substrate-host/src/confinement/oci.rs:741-769`)
  and reports no limit.
- `systemd` enforces the timeout and the CPU budget itself and sets `memory.oom.group`
  (`crates/substrate-host/src/confinement/systemd.rs:14`), and reports `memory_oom_kills: None`
  (`crates/substrate-host/src/confinement/systemd.rs:1071`).
- Only `pod` sets `memory_oom_kills` (`crates/substrate-host/src/confinement/pod.rs:1261-1273`).
- Output over a run's bound does not end the run. It is truncated and already stated on
  `RunOutput` (`crates/substrate-host/src/confinement/mod.rs:709-719`), so it is not a limit here.

## What changes

1. **A design document first** (`AGENTS.md` invariant 8), `docs/design/36-limit-hits.md`. It
   decides the field (name, a closed enum), which limit wins when two are true (the host driver's
   order is memory, CPU, timeout, `process.rs:3296-3313`), the value for the aperture byte ceiling
   and for output backpressure, and one row per adapter: which limits it observes and from what.
   `macos` and `local` serve no `limits` family and report none.
   `story:audit-records-run-end-and-limit` and `story:process-count-limit-hit-is-observed` add
   their sections to the same document.
2. `spec/confinement/confinement.yaml` states the field and a scenario per limit.
3. `linux` maps the host observation's refusal code. `systemd` reports the timeout and the CPU
   budget it enforced and a memory limit from its cgroup's `memory.events`. `oci` reports the
   memory limit when the runtime states `OOMKilled`, and its timeout. `pod` reports the memory
   limit and its deadline from the container status. `linux` and `systemd` report
   `memory_oom_kills`.
4. **The host driver carries the OOM count to the adapter.** Today it reads the count of an
   unmeasured run into a local, `unmeasured_oom_kills`
   (`crates/substrate-host/src/process.rs:3128`), and the run's cgroup is removed afterwards, so
   the `linux` adapter has nothing to read. This story adds a host-internal member for end-of-run
   counters to `ExecObservation` (`crates/substrate-host/src/observation.rs:19-34`), outside the
   wire `resource`, and fills it with the OOM count for every ended exec.
   `story:process-count-limit-hit-is-observed` adds the process count to the same member.

## Acceptance

In the delegated lane, three runs on the `linux` adapter that exceed their memory bound, their CPU
budget and their timeout each end with an observation that names that limit, and a run that exits
0 names none; before this story `RunObservation` has no such field.

Checks:

- The design document is merged before any code of this story.
- The memory case's observation has `memory_oom_kills` of at least 1 on `linux`, and the same
  case on the `systemd` lane (`crates/substrate-host/tests/ess_confinement_systemd.rs`) has it on
  `systemd` around `local`.
- A unit test maps every refusal code the host driver writes on an ended exec
  (`exec.memory-limit`, `exec.cpu-limit`, `exec.timeout`, `exec.aperture-byte-limit` and the
  output backpressure code) to the value the design names for it.
- On the `systemd` lane, runs on `systemd` around `local` that exceed their timeout and their CPU
  budget name those limits.
- On the `oci` lane (`SUBSTRATE_OCI_LANE`) and on the `pod` lane (`SUBSTRATE_POD_LANE`), a run
  that exceeds its memory bound names the memory limit, and one that exceeds its timeout names the
  wall-time limit.
- No member of the wire `Exec` resource changes; `cargo xtask check-bundles` passes unchanged.
- An adapter that does not observe a limit leaves the field absent; no adapter derives it from an
  exit code alone (invariant 3).
- `crates/substrate-host/tests/confinement-public-items.txt` is regenerated in the same change.

## Boundaries

- No audit record: `story:audit-records-run-end-and-limit`.
- No process-count field: `story:process-count-limit-hit-is-observed`. A process-count hit does
  not end a run.
- No wire change. The wire observation already carries `refusal`
  (`crates/substrate-wire/src/lib.rs:2233-2245`).

## Order

After `story:executable-deny-list` only because both change
`crates/substrate-host/src/confinement/linux.rs`, `macos.rs` in the same directory,
`spec/confinement/confinement.yaml` and `crates/substrate-host/tests/confinement-public-items.txt`.
Nothing in this story reads what that one produces.

## Scope

- `docs/design/36-limit-hits.md` (new). Inferred.
- `spec/confinement/confinement.yaml`, `spec/confinement/scenarios`, `spec/confinement-suite.json`.
  Inferred.
- `crates/substrate-host/src/confinement/mod.rs`. Cited: `:696-706`.
- `crates/substrate-host/src/confinement/linux.rs`. Cited: `:507-526`.
- `crates/substrate-host/src/process.rs`. Cited: `:3128`, `:3280-3322`.
- `crates/substrate-host/src/observation.rs`. Cited: `:19-34`.
- `website/docs`. Inferred: `cargo xtask check-docs` holds the site's contract tables to the spec.
- `crates/substrate-host/src/confinement/systemd.rs`. Cited: `:14`, `:1071`.
- `crates/substrate-host/src/confinement/oci.rs`. Cited: `:741-769`, `:817`.
- `crates/substrate-host/src/confinement/pod.rs`. Cited: `:1261-1273`.
- `crates/substrate-host/src/confinement/local.rs`, `crates/substrate-host/src/confinement/macos.rs`.
  Cited: `local.rs:200`, `macos.rs:520` (the struct literal gains a field).
- `crates/substrate-host/tests/confinement-public-items.txt`. Cited: `:1-3`.
