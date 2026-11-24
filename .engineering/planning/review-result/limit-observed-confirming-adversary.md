---
format: aep.planning-md/3
id: review-result:limit-observed-confirming-adversary
kind: review-result
status: active
title: 'Confirming adversary pass: a run''s observation names the limit that ended it'
relations:
- reviews: story:run-observation-names-the-limit-that-ended-it
revision: 1
---
# Confirming adversary pass: a run's observation names the limit that ended it

2026-11-24, on the commit `9a6ab233f` of the branch `feat/run-observation-names-its-limit`
(design 36, story 1, second change). The adversary added three cases, test code only, and did
not edit the implementation. Verdict: three cases red. Host library selection (`confinement`,
`observation`, `process::`) executed 299 to 302, red 3. S1: 0. S2: 1. S3: 4.

Nothing of this branch is released.

## Held

- D4's order: `LIMIT_ORDER` and `named_limit` follow it, and `linux` records its refusals in
  that order; `exec.metrics-unavailable` above memory is what D6 states.
- No reaped pid is signalled: in `impose`, `stop`, `end_stalled`, the signal path and the end
  task of `systemd`, `exited` is set under the lock before the reap.
- No lock cycle between `imposed`, `exited`, `ended_run` and `reaped`.
- `exited_unreaped` answers `false` for pid 0, a pid that is no child and a negative pid, and
  leaves a running child unreaped (`WNOWAIT`).
- The deadline arm does not starve; a stall or an aperture ceiling it missed is recorded after
  the wait.
- A deadline closes live output before its kill, so no stall follows it.
- `EndOfRun` is `#[non_exhaustive]`; outside the host crate it appears only as `None`.

Not established by the pass: the delegated `linux` and `systemd` lanes, the `oci` and `pod`
lanes, and macOS (`waitid` with `WNOWAIT` and `WNOHANG`).

## Findings and what was done

1. S2. Rule asked: an exec that a limit ended, whose output then stalls after its exit, names
   that limit. Case `an_exec_the_oom_kill_ended_whose_output_stalled_after_its_exit_names_memory`;
   a second case holds the aperture ceiling (`EgressBytes`).
2. S3. Rule asked: on `linux` a stall after the program's own exit is marked after the exit,
   whether or not the supervision has taken the exit. Case
   `a_stall_after_the_programs_own_exit_is_marked_after_the_exit_before_the_supervision_sees_it`.
3. S3. Rule asked: a stall's kill that comes after the embedder's `KILL` answers that it ended
   nothing. Case `a_stalls_kill_after_the_embedders_kill_answers_that_it_ended_nothing` on
   `systemd`.
4. S3. The changelog listed `oci` among the adapters that name a stall by the leader's exit; it
   names one by whether the runtime client had returned.
5. S3. The `oci_limit` documentation, D7's `oci` note and the changelog gave two ranges of exit
   status; with `OOMKilled` every status outside 0 to 128 is read as a signal.

Done in the branch's third commit, on decisions of the session that works this repository,
reported to the operator:

- `linux`: a stall marked after the exit names `EgressBytes` where the driver reached an
  aperture ceiling (`EndOfRun` gains the public field `egress_ceiling_reached`), then `Memory`
  where the OOM count is above zero, else none. The OOM count names `Memory` also where the
  run's other counters could not be read. D5 and D6 say so.
- `linux`: the mark is asked under the lock the reap holds, from the run's unreaped child; a
  stall flagged after the supervision began to end the run by a bound of its own is not marked.
- Finding 3 on every adapter with a stall kill: `systemd` reads `stopped`; `local` and `macos`
  record the embedder's `KILL` in `stopped` in one shared `signal` body; `oci` sets `stopped`
  before its runtime-client call. `pod` has no stall kill: it refuses live output.
- Findings 4 and 5: the text states what the code reads.

Seven cases, the pass's three among them under rule-form names. Runs of the change, release
profile, in the build pool: the host library selection 306 of 306; `ess_confinement_local` and
`consumer_surface` 48 of 48; `confinement_public_items` 18 of 18; the daemon's and the
namespace driver's library cases 343 of 343.
