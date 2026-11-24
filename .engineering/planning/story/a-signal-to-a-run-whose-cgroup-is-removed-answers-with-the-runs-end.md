---
format: aep.planning-md/3
id: story:a-signal-to-a-run-whose-cgroup-is-removed-answers-with-the-runs-end
kind: story
status: draft
title: A signal to a run whose cgroup is removed answers with the run's end
tags:
- defect
scope:
- confidence: cited
  path: crates/substrate-host/src/process.rs
revision: 2
---
# Story: a signal to a run whose cgroup is removed answers with the run's end

## Outcome

The host driver's `signal` answers with the run's terminal observation when the run's cgroup is
already removed, whichever of its steps finds the cgroup gone.

## Where the requirement comes from

The session that works this repository, from the forge: the `Confinement lane` job of
`release-lanes.yml` on `main` at `1d7be7b17` (2026-11-24) ended with 91 of 92 cases of
`ess_confinement_linux` passed. Collection rule *The local gate equals the forge*: a forge run that
goes red gets the change that keeps it from recurring. Stakeholder: the operator's session.

## The rule

The driver removes a run's cgroup only after it proved the cgroup empty, and the kernel removes
no cgroup that holds a process. A removed cgroup therefore has no process left to signal. A
signal that finds the cgroup removed

- records no delivered signal and no cancellation for the step that reached nothing,
- waits for the record to turn terminal, at most 5 s as before, and
- answers with that observation.

Only a removed cgroup is read this way. A cgroup directory that is there and takes no kill, as
on a kernel without `cgroup.kill`, fails the signal `exec.cgroup-write-failed` as before
(invariant 3), and a directory without its `cgroup.procs` is answered as before.

The `systemd` adapter already holds the same rule for its own kill (`kill_tree` in
`crates/substrate-host/src/confinement/systemd.rs`).

## What changes

`ProcessRuntime::signal` in `crates/substrate-host/src/process.rs` asks the cgroup through
`Cgroup::existing_or_removed`, and each of its three steps (the kill, the signal to every
process, the kill after the grace) goes through `Cgroup::reached`.

## Design

No wire schema, route, refusal code or fact changes. No design document is owed: the answer for
a run that has ended is the one `signal` gives a terminal record today.

## Acceptance

Three cases in the unit tests of `crates/substrate-host/src/process.rs`, portable, on a paused
clock, each under 1 s:

- `a_signal_to_a_run_whose_cgroup_is_removed_answers_with_the_runs_end`: `Term` and `Kill`,
  cgroup removed before the signal; the answer is the run's end, no delivery and no cancellation
  recorded.
- `a_kill_after_the_grace_to_a_removed_cgroup_answers_with_the_runs_end`: the cgroup is there
  for the signal and removed within the grace; `Term` is recorded, no cancellation.
- `a_kill_a_present_cgroup_does_not_take_stays_a_failure`: a present cgroup without
  `cgroup.kill` fails `exec.cgroup-write-failed`, at once and after the grace; without
  `cgroup.procs` the answer is not-found.

The delegated lane's `ESS confinement linux adv live term` runs unchanged.
