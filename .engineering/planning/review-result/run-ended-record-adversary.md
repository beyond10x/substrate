---
format: aep.planning-md/3
id: review-result:run-ended-record-adversary
kind: review-result
status: active
title: Adversary pass on the run_ended audit record
relations:
- reviews: story:audit-records-run-end-and-limit
revision: 1
---
# Adversary pass on the `run_ended` audit record

2026-11-24, on the commit `959c8206e` of the branch `feat/run-ended-audit-record`
(`story:audit-records-run-end-and-limit`, design 36 § 4). The adversary added four cases, test
code only, in `crates/substrate-host/tests/ess_confinement_local.rs`, and did not edit the
implementation or a document. Verdict: one case red. `ess_confinement_local` executed 51 to 55,
red 1. S1: 0. S2: 2. S3: 3.

Nothing of this branch is released or on `main`.

## Held

- A run still running when its current-thread runtime is dropped gets one `run_ended` with
  `state: unknown` and no exit, after its `run_started`, and nothing more
  (`a_run_live_when_its_current_thread_runtime_is_dropped_writes_one_unknown_run_ended`, run).
- A sink that panics on one run's `run_ended` stops neither `wait` nor the next run's record
  (`a_sink_that_panics_on_run_ended_stops_neither_wait_nor_the_next_record`, run).
- On `local` the record names the limit, the exit and the state its observation names, for a
  stalled run and for a run that ended before the stall
  (`a_local_run_ended_record_names_the_limit_its_observation_names`, run, 2.1 s).
- `local`, `systemd` around `local`, `macos` and `oci` build the record from the phase value
  they publish; `pod` writes on the first publication only (read).
- `linux`: the exec's terminal state and its end-of-run member are set under one lock, and the
  adapter never retires an exec, so the observation the end task reads is the one `observe` and
  `wait` return (read).
- One record per run: the write takes the value, the value is not `Clone`, and its drop writes
  nothing after a write (read, and the committed unit cases).
- Order: the value exists only after `run_started` was written; on `linux` and `systemd` around
  `linux` no fallible step lies between that and the spawn (read).
- A thread that is panicking writes nothing; the sink is called with no adapter lock held (read).
- Serialization, `tests/confinement-public-items.txt` and the changelog's case counts agree with
  the code (read).
- Mutant, run: the record written on the end task's own thread is killed by
  `a_sink_that_blocks_on_run_ended_does_not_stop_wait_on_a_current_thread_runtime`.

Not established by the pass: the delegated `linux` and `systemd` cases (a job of the build pool
does not run inside a delegated scope of the workstation, so the `linux` acceptance case printed
its `ABSENT (` line), the `oci`, `pod` and `macos` cases (read, not run). Mutants not run: the
record's `memory_oom_kills` always absent; the `linux` end task spawned for a profile without a
sink; the drop writing nothing.

## Findings and what was done

1. S2. Rule asked: a sink that blocks on `run_ended` delays neither `wait` nor another run,
   whatever the size of the runtime's blocking pool. Case
   `a_sink_that_blocks_on_run_ended_delays_no_other_run_on_a_one_thread_blocking_pool`.
2. S2. Rule asked: a case that runs in the gate's test step is timed before it is added and
   ends by its own assertion before the 60 s bound. `gate.yml` sets `SUBSTRATE_OCI_LANE` on the
   test job, so the new `oci` case runs in that step.
3. S3. Rule asked: the documentation of `ConfinementProfile::audit` lists every record a sink
   receives.
4. S3. Rule asked: every assertion of a case can fail.
   `without_a_sink_the_run_end_value_writes_nothing` reads a list no sink feeds.
5. S3. Rule asked: a gate-step case holds the limit of a `run_ended` an adapter's end task
   writes. The mutant "the end task observes without the live queue" passed the 51 committed
   cases; the pass's limit case holds it for `local`.

Done in the branch's next commit, whose message states each.

Run of the pass, release profile, in the build pool: `ess_confinement_local` 55 run, 54 passed,
1 failed (the case of finding 1); without the four added cases 51 of 51.
