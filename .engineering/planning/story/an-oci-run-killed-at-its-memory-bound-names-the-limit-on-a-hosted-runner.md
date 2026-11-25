---
format: aep.planning-md/3
id: story:an-oci-run-killed-at-its-memory-bound-names-the-limit-on-a-hosted-runner
kind: story
status: draft
title: An oci run killed at its memory bound names the memory limit on a hosted runner
summary: 'Hosted gate, second sighting: signal 9 with limit None after the one-second re-read of the runtime''s state'
tags:
- defect
- flaky
relations:
- decomposes: epic:runs-are-governed-and-limit-hits-are-events
- informed_by: story:run-observation-names-the-limit-that-ended-it
revision: 1
---
# Story: an oci run killed at its memory bound names the memory limit on a hosted runner

## Outcome

On the `oci` adapter, a run the kernel killed at its memory bound ends with
`limit: Some(Memory)`, or the observation says that the limit could not be read and why. The
gate's case `an_oci_run_over_its_memory_bound_names_the_memory_limit` passes on a hosted runner
in every run, without a retry.

## Where the requirement comes from

The gate job `Gate: tests and Clippy` of pull request 302 failed once on 2026-11-25 in
`an_oci_run_over_its_memory_bound_names_the_memory_limit`
(`crates/substrate-host/tests/ess_confinement_oci.rs:1297`), run 45594091184, attempt 1, job
139144273381. The pull request changes no code of the host crate. The same tree passed that job
on pull request 301. Stakeholder: the Runbook consumer, whose requirement is that every limit hit
is visible as an event.

## Observed

```
ESS confinement oci limit memory: the run ended after 1.61908603s
assertion `left != right` failed: ESS confinement oci limit memory: the runtime's state held no
OOMKilled within a second of the exit: RunObservation { …, adapter: Oci, …, state: Ended,
exit: Some(RunExit { code: None, signal: Some(9) }), limit: None, memory_oom_kills: None,
process_limit_hits: None }
```

This is the second sighting. The first, on a hosted runner on 2026-11-24, is recorded in
[design 36](../../../docs/design/36-limit-hits.md) under what is not known; the monitor then
made one read. Since that day the monitor reads the runtime's state again for one second when
the first read holds no `OOMKilled` (D7's `oci` rule). The run above ended with signal 9 and the
state held no flag within that second. 3,774 of the run's 3,775 cases passed.

## Why it matters

- A consumer reads `limit` to tell a memory-bound kill from another kill. Here the kill is
  reported as signal 9 with no limit, so the limit hit is not visible.
- D7 forbids deriving a limit from an exit or a signal alone, so the adapter cannot fill the gap
  from the signal.
- The case fails the gate of a pull request that does not touch it. A re-run of the job is the
  only way past it today.

## Not known

- Whether the runtime's flag was late by more than a second or never set. Design 36 names both
  as possible: Docker sets the flag when the OOM event arrives, and the watcher sends no event
  when the cgroup is removed before it reads `memory.events`.
- How often the case fails on a hosted runner. Two sightings are recorded; no rate was measured.
- Whether another source the adapter can read states the kill: the runtime's event stream, or
  the container's cgroup counter before the runtime removes it.

## What changes

1. Measure first, on a hosted runner and outside the gate: the case's shape repeated, with the
   time from the exit to the flag, and the count of runs with no flag at all. The run is recorded
   in the planning store, never added as a test or a CI step.
2. From the measurement, amend design 36 D7's `oci` rule before code (invariant 8): a longer
   read, a second source, or an observation that says the limit could not be read.
3. The gate's case keeps its assertion. It is not loosened and gets no retry.

## Acceptance

A case that forces the state the hosted runner showed (signal 9, no flag within the read) is red
before the change and green after it, and the measurement of step 1, repeated after the change,
records no run of the case that ends without a named limit or a stated reason.
