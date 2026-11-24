---
format: aep.planning-md/3
id: review-result:run-ended-record-confirming-adversary
kind: review-result
status: active
title: 'Confirming adversary pass on the run_ended record: a thread of its own'
relations:
- reviews: story:audit-records-run-end-and-limit
revision: 1
---
# Confirming adversary pass on the run_ended record: a thread of its own

2026-11-24, on the commit `c12141e07` of the branch `feat/run-ended-audit-record` (design 36
§ 4, D15 to D25, with D17 and D20 as amended that day). The adversary added three cases, test
code only: one in the test module of `crates/substrate-host/src/confinement/mod.rs`, because
`deliver_with` is private, and two in `crates/substrate-host/tests/ess_confinement_local.rs`. It
changed no implementation line and no document. Verdict: needs change. Cases executed 55 to 57
(`ess_confinement_local`) and 24 to 25 (`confinement::tests`), red 3. S1: 1. S2: 1. S3: 3. All
five introduced by the commit under review.

Nothing of this branch is released or pushed.

## Held

- The record is written at most once where the system refuses the thread: the job is dropped
  unrun on the caller and the slot still holds the write (read).
- Where the blocking pool cannot start a thread, the panic is caught, the caller takes the
  write, and the queued job finds the slot empty (read; the condition was not induced).
- On a pool that is shutting down the record is written once, on the caller, as the full ended
  record (run).
- `run_started` is written before the value exists, so `run_ended` cannot come before it (read).
- No lock is held across the sink call (read).
- A profile without a sink starts no task and no thread, on `linux` and `systemd` too (read).
- One thread per record, without a bound for a sink that never returns, is stated in § 13
  (read).
- The changelog's counts of cases match the code (read).
- A blocking sink on a one-thread pool and on a current-thread runtime, and a panicking sink:
  green (run).
- Mutants killed: the thread without its name, the calling-thread fallback that forgets the
  write, every record on the blocking pool. Not run: the runtime branch returning without
  queuing the job, the `catch_unwind` around `spawn_blocking` removed, the write inline on the
  end task (killed in the first pass).

Not established by the pass: the delegated `linux` and `systemd` cases and the `oci`, `pod` and
`macos` cases were read, not run.

## Findings and what was done

1. S1. Rule asked: design 36 Q5 A, the stakeholder's answer, "Every `run_started` in a process
   that returns from its runtime has its `run_ended`." Case
   `a_run_that_ended_has_its_run_ended_in_the_sink_once_its_runtime_returned`. The adversary put
   the choice to the coordinator: change the document or the code. Decided: both rules hold and
   the code changes. The record keeps its own thread (D20), the run's end task awaits the write
   on a channel, and a runtime that drops the task before the sink returned joins the record's
   thread. D20, D22, Q5's cost line and §§ 10, 12 and 13 are amended.
2. S2. Rule asked: on every path the record is written once and a sink that panics is contained
   (D20). Case
   `a_run_ended_a_shut_down_pool_cannot_take_is_written_once_by_the_caller_and_contained`.
3. S3. Rule asked: every record of one run reaches the sink in the same context. Case
   `a_sink_that_reaches_the_runtime_on_run_started_reaches_it_on_run_ended`. Design 36 now states
   the rule: the record's thread enters the runtime that was current at the hand-off.
4. S3. Rule asked: the `oci` case ends by its own assertion inside the test step's 60 s. Its
   start and its record share what is left of 50 s after the adapter opened. Read, not run.
5. S3. Rule asked: a document states its condition. The `AuditSink` documentation, D20 and the
   changelog name `panic = "unwind"`.

Done in the branch's next commit, whose message states each. The three cases are kept under
their names without the `adv_` prefix.

Run of the pass, release profile, in the build pool: `ess_confinement_local` 57 run, 55
passed, 2 failed; the library's `confinement::tests` 25 run, 24 passed, 1 failed; the three
failures are the cases of findings 1 to 3.
