---
format: aep.planning-md/3
id: review-result:process-count-adversary
kind: review-result
status: active
title: 'Adversary pass on the process count: refused creations on the observation and the record'
relations:
- reviews: story:process-count-limit-hit-is-observed
revision: 1
---
# Adversary pass on the process count: refused creations on the observation and the record

2026-11-24, on the commit `e0a9eec1c` of the branch `feat/process-count-limit-hits` (design 36
§ 3, D10 to D14), read in the tree of its merge `f21af948d`. The adversary added four cases,
test code only, in the test modules of `crates/substrate-host/src/confinement/linux.rs`,
`confinement/systemd.rs` and `process.rs`, and changed no implementation line and no document.
Verdict: approve. Cases executed 476 to 480, red 0. S1: 0. S2: 0. S3: 6, all introduced by the
commit under review.

Nothing of this branch is released or pushed.

## Held

- Both readers of `pids.events` give the count or absence for an empty file, a missing `max`, a
  value that is no number, a value past `u64`, `-1`, trailing whitespace, CRLF and keys in any
  order; a missing file or a removed cgroup gives absence (run).
- `linux` reads the cgroup that carries `pids.max` and that the leader joins; `systemd` around
  `local` reads the run cgroup it wrote `pids.max` into; `systemd` around `linux` passes the
  inner observation through unchanged (read).
- The observation and the `run_ended` record are built from the same stored end (read).
- A measured exec's terminal usage and its end of run come from one sample; no file under
  `contracts/` or of the wire crate changed (read and run).
- D12: neither `EndRead::of` on `systemd` nor `ended_limit` on `linux` reads the count (run).
- `oci`, `pod`, `local` and `macos` set absence; `systemd` around `local` refuses a start whose
  `pids.max` cannot be written, so no run exists whose count could be read as `0` (read and
  run).
- The member is omitted when absent, and the JSON of design 36 § 4 matches the unit case member
  for member (run).
- The lane cases on the hosted runner: each would pass, each is bounded (10 s, 20 s, 5 s and a
  2 s timeout), each prints `ABSENT (` with its reason where a prerequisite is missing, and
  `scripts/delegated-lane.sh` names all 7 lines (read, not run).
- The changelog entry and the rows of design 36 §§ 3 and 9 match the code (read).
- Mutants killed: the `linux` carry set to absence (by the pass's own case only), a failed read
  as `0` in the driver, `local` set to `0`. Not run: `oci` or `pod` set to `0`, the running read
  removed from `linux` or from `systemd`, the `systemd` end task's read set to absence.

Not established by the pass: the delegated `linux` and `systemd` cases, `consumer_surface`'s
governed run and the `macos` cases were read, not run.

## Findings and what was done

1. S3. Rule asked: a document says what the counter counts. D10 and the rustdoc call it the
   creations the run's bound refused; the kernel's `max` of `pids.events` also counts a refusal
   by a bound above the cgroup on a kernel without `pids.events.local`, and by a bound of a
   cgroup beneath it on a newer one. Read, not run.
2. S3. Rule asked: each row of D14 is held by a case. No case asserted absence on `oci` or
   `pod`.
3. S3. Rule asked: the `linux` carry of the count into the observation, and D12 on `linux`, are
   held at the test step. Cases
   `the_ended_linux_observation_carries_the_drivers_end_of_run_count` and
   `a_count_of_refused_creations_names_no_limit_on_linux`.
4. S3. Rule asked: D13 says how a running count relates to the ended one. A process left in the
   tree can be refused after the end's read and before the kill is complete. Read.
5. S3. Rule asked: the two readers of `pids.events` agree. They differ on forms the kernel does
   not write (an extra field, a tab, a leading space). Read and run.
6. S3. Rule asked: a lane case states what it depends on. The over-the-bound and governed cases
   on `linux` hold only while three processes exist before `xargs` under a bound of four. Read.

Done in the branch's next commit, whose message states each. The four cases are kept without
the `adversary_` prefix.

Run of the pass, release profile, in the build pool: the host library's `process::` and
`confinement::` cases with `ess_confinement_local`, `consumer_surface`, the public-items target
and the two delegated targets' cases that run without a delegated root, 480 run, 480 passed, in
26.7 s.
