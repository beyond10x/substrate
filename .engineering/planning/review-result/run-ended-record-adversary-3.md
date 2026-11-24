---
format: aep.planning-md/3
id: review-result:run-ended-record-adversary-3
kind: review-result
status: active
title: 'Third adversary pass on the run_ended record: the awaited write and the join'
relations:
- reviews: story:audit-records-run-end-and-limit
revision: 1
---
# Third adversary pass on the run_ended record: the awaited write and the join

2026-11-24, on the commit `6ac3db6b0` of the branch `feat/run-ended-audit-record` (design 36
§ 4, D20 and D22 as amended that day, Q5 A). The adversary added two cases, test code only: one
in the test module of `crates/substrate-host/src/confinement/mod.rs` and one in
`crates/substrate-host/tests/ess_confinement_local.rs`. It changed no implementation line and no
document. Verdict: needs change. Cases executed 57 to 58 (`ess_confinement_local`) and 26 to 27
(`confinement::tests`), red 1. S1: 0. S2: 1. S3: 1. Both introduced by the commit under review.

Nothing of this branch is released or pushed.

## Held

- The five findings of `review-result:run-ended-record-confirming-adversary` are closed in the
  code: the guard joins (run, by two mutants and the commit's case), a shut-down pool leaves one
  contained write to the caller (run), the record's thread enters the runtime (run, by a
  mutant), the `oci` case's 50 s (read) and the `panic = "unwind"` condition (read).
- Every path of `deliver_with` writes once, releases the end task's wait and contains a sink's
  panic: thread started, thread refused with the pool taking the job, with the pool shutting
  down, with the pool unable to start a thread, and no runtime (read; the last pool condition
  was not induced).
- No end task is dropped while its runtime keeps running: every adapter detaches its end or
  monitor task, and only readers are aborted (read).
- Two runs that end at once on a current-thread runtime both wait on their channel and hold no
  thread (read).
- The commit's statements on tokio 1.53.1 hold: a current-thread runtime's drop is not bounded
  by `shutdown_timeout`, and a multi-thread runtime's workers run on its blocking pool (read).
- `pod`: the observation is taken before the spawn, a second publication writes nothing, and a
  monitor dropped after `publish` does not lose the record (read).
- A profile without a sink starts no task, no thread and no channel wait (read, and one unit
  case run).
- The shared lane helper's bound now covers the start and the record together, which is
  stricter for every lane that uses it (read).
- The changelog's counts of cases and the public-items list match the code (read).
- Mutants killed: the join replaced by a detach, the end task not awaiting the write, the
  record's thread not entering the runtime. Not run: the panicking check of the guard's drop
  removed, the `catch_unwind` around `spawn_blocking` removed, `pod` writing inline.

Not established by the pass: the multi-thread form of finding 1 was read, not run; the
delegated `linux` and `systemd` cases and the `oci`, `pod` and `macos` cases were read, not run.

## Findings and what was done

1. S2. Rule asked: the documents say which sink holds a runtime's drop. A sink that waits on
   the runtime the record's thread entered (a timer or I/O of it) does not return once that
   runtime's drop has begun, and the drop does not return. The adversary put three ways to the
   coordinator: state it, do not enter the runtime, or bound the join. Decided: the join (Q5 A)
   and the entered runtime (a sink that hands work to it) stay; the `AuditSink` documentation
   and design 36 state that a sink may hand work to the runtime and must not wait on it, and
   the case is kept as a `limit_` case that holds the stated behaviour by its figures.
2. S3. Rule asked: D22, "A run that ended before the task was dropped is D20's". On `pod` the
   write was handed to a spawned task, so a runtime shut down before that task's first poll
   could record an ended run as `unknown`. Read. `publish` takes the value of D16 without
   waiting, hands the ended record to its thread before it returns and spawns only the wait.
   Where the value has not arrived yet (`start`'s thread held between spawning the monitor and
   handing it the value), the spawned task still takes the value and writes; design 36 D20
   states that one path.

The pass's second case, which holds the join without depending on thread timing, is kept
without the `adv_` prefix. Done in the branch's next commit, whose message states each.

Run of the pass, release profile, in the build pool: `ess_confinement_local` 58 run, 57
passed, 1 failed (the case of finding 1); the library's `confinement::tests` 27 of 27.
