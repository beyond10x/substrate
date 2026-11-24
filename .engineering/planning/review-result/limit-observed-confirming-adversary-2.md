---
format: aep.planning-md/3
id: review-result:limit-observed-confirming-adversary-2
kind: review-result
status: active
title: 'Second confirming adversary pass: a run''s observation names the limit that ended it'
relations:
- reviews: story:run-observation-names-the-limit-that-ended-it
revision: 1
---
# Second confirming adversary pass: a run's observation names the limit that ended it

2026-11-24, on the commit `5e4774c5b` of the branch `feat/run-observation-names-its-limit`
(design 36, story 1, third change). The adversary added one case, test code only, and did not
edit the implementation. Verdict: one case red, reached only by the case's own stand-in
channel; no rule broke in a run of the driver. Host library selection (`confinement`,
`observation`, `process::`) executed 306 to 307, red 1. S1: 0. S2: 0. S3: 2.

Nothing of this branch is released.

## Held

- The mark is decided under the lock the reap holds; every bound branch clears the pid before
  its kill; a stall flagged twice is guarded; no stall can be flagged before the pid is stored.
- A marked exec carries `session.output-backpressure` and no other code;
  `egress_ceiling_reached` is read only under the mark; an OOM count that could not be read
  names nothing.
- Every adapter's `KILL` closes live output before it sets `stopped`, so a failed `KILL` hides
  no later stall. `TERM` and `INT` do not set `stopped`, and no text says they do.
- The new lock is taken in four functions, nothing waits while holding it, and no other lock is
  held when it is taken. No reaped pid is signalled.
- `EndOfRun` is `#[non_exhaustive]` with three public fields; `confinement_public_items` 18 of
  18.
- D5, D6, D7, § 12, the `oci_limit` documentation and the changelog say what the code does,
  but for finding 2.

Not established by the pass:

- One order no stand-in can force: a stall is flagged, the supervision's tick begins to end the
  run by a bound before the stall asks, and the child had exited on its own with its exit not
  yet taken. The stall is then not marked and the run names the stall.
- No unit case drives `run_child`, so no case of the selection holds the two lines that copy
  the mark and `egress_ceiling_reached` into the end-of-run record. The delegated lane does.
- Not run: the delegated `linux` and `systemd` lanes, the `oci` and `pod` lanes, macOS.

## Findings and what was done

1. S3. Rule asked: a stall flagged after the supervision began to end the run by a bound of
   its own is not marked after the exit, also once the supervision has reaped the child it
   killed. Case
   `a_stall_after_a_bounds_reap_is_not_marked_after_the_exit`.
   Done: `end_by_bound` sets `ended_by_bound` under the lock, and `exited_on_its_own` answers
   `false` from then.
2. S3. § 7 said nothing in the confinement module names `EndOfRun` or `ExecObservation`; two
   private functions of `linux` do. Done: the sentence says no public item names them.

Run of the change, release profile, in the build pool: the host library selection 307 of 307.
