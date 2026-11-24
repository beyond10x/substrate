---
format: aep.planning-md/3
id: review-result:run-ended-record-adversary-4
kind: review-result
status: active
title: 'Fourth adversary pass on the run_ended record: the sink rule and pod''s handed-over record'
relations:
- reviews: story:audit-records-run-end-and-limit
revision: 1
---
# Fourth adversary pass on the run_ended record: the sink rule and pod's handed-over record

2026-11-24, on the commit `aab3c0cd0` of the branch `feat/run-ended-audit-record` against its
parent `6ac3db6b0` (design 36 § 4, D20 and D22 as amended that day). The adversary added three
cases, test code only: one in `crates/substrate-host/src/confinement/pod_cleanup_force_deleted.rs`
and two in `crates/substrate-host/tests/ess_confinement_local.rs`. It ran one mutant of
`pod.rs` and restored the file (same SHA-256 before and after). Verdict: needs change. Cases
executed 180 to 181 (the library's `confinement::`) and 58 to 60 (`ess_confinement_local`),
red 1. S1: 0. S2: 0. S3: 1, introduced by the commit under review.

Nothing of this branch is released or pushed.

## Held

- `publish` meets a value that has not arrived only where `start`'s thread is held past the
  monitor's first poll (500 ms) between spawning the monitor and handing it the value, on a
  multi-thread runtime; a Pod that ended before `start` returned is seen only after that poll
  (read).
- A start that failed after it spawned the monitor writes no `run_ended`; that state is reached
  only by a panic out of the `run_started` sink, as on the parent commit (read).
- The record handed over on the monitor's task spawns a thread, does not block the task and
  does not panic. Where the system refuses the thread and the pool has no thread to spare, the
  job stays queued. The sink runs on the monitor's task only where the thread is refused and
  the runtime's blocking pool is already shut down (read in tokio 1.53.1
  `runtime/blocking/pool.rs`, lines 393 to 438; thread refusal not induced).
- The `limit_` case cannot outlast its process, and it does not pass for a wrong reason: the
  sink was entered with `run_ended`, and a control case shows that the same sink returns, and
  the drop returns, while `Runtime::block_on` still drives the timer (run, 0.410 s).
- A sink that spawns on the runtime it finds current and sends on a channel while that
  runtime's drop joins it returns without a panic; the drop returns, the record is in the
  sink, the send arrives and the spawned body never runs (read, then run, 0.109 s).
- The commit's `pod` unit case goes red against the earlier form of `publish` (run, one
  mutant, killed).
- The changelog's counts match the code: thirteen unit cases of the record, two `pod` unit
  cases, twelve `ess_confinement_local` cases (read).

Not established by the pass: a multi-thread runtime's drop with a waiting sink; the `pod`
lane, the delegated `linux` and `systemd` cases, `oci` and `macos` (none can run in the build
pool). That the `limit_` case fails only when the test's main thread wakes 200 ms late is
inferred, not shown.

## Findings and what was done

1. S3. Rule asked: D22 says what is recorded where the publication comes before the value of
   D16. The design and the changelog said a runtime dropped right after `pod`'s publication
   cannot record the run `unknown`, with no exception; on the path where the value had not
   arrived, the task that waited for it could be dropped before its first poll, and the
   record said `unknown` (run, the pass's first case, red). Decided: on `pod` no task waits
   for the value. `start` and the monitor's publication share a cell, and whichever comes
   second hands the ended record to its thread, so D22 holds on that path too. The pass's case
   is kept and holds the ended record.

Two notes of the pass, both taken: the sink rule's wording names the point from which a
current-thread runtime no longer drives a sink's wait (its `block_on` returned), and the
documentation of `publish` names the one condition under which a sink runs on the monitor's
task. The pass's two green cases are kept without the `adv4_` prefix. Done in the branch's
next commit, whose message states each.

Runs of the pass, release profile, in the build pool: the library's `confinement::` 181 run,
180 passed, 1 failed (the case of finding 1); `ess_confinement_local` 60 of 60.
