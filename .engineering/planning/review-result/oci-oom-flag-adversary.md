---
format: aep.planning-md/3
id: review-result:oci-oom-flag-adversary
kind: review-result
status: active
title: Adversary pass on the oci monitor's second read of an ended container's state
relations:
- reviews: story:run-observation-names-the-limit-that-ended-it
revision: 1
---
# Review result: adversary pass on the oci monitor's second read of an ended container's state

Reviewed: commit `eb60fe69a` on `release/0.27.0`, design 36 D7's `oci` rule as amended on
2026-11-24. One adversary agent in a tree of its own, 2026-11-24. It wrote five cases and tried
three mutants. Findings: no S1, no S2, five S3, all introduced by the commit, all closed in the
commit that carries this record.

## Held

- An embedder's kill, the monitor's deadline, a flag on a later read and a flag that never comes
  give what the rule says. Run: four cases that start `monitor` with a stand-in runtime script,
  no container.
- Every way an embedder ends a run goes through `signal`, which sets its flag before its runtime
  call. Read.
- `Int` and `Term` do not set the flag; a container runs under `--init`, so they end with 130
  or 143, not 137. Read.
- The second read cannot change D4's order: where it runs, neither the stall nor the deadline
  sent a kill, so the end names `Memory` or nothing. Read.
- A later read that fails keeps the state read before it; no first state reads nothing. Run,
  the commit's own case.
- The order after the reads is unchanged: remove, the stdin writer's end, the phase, the record.
  Read.
- `ess_confinement_oci`, `adversary_oci` and `security_review_oci` hold no timing assertion the
  delay breaks. Read; 36 of 36 with the lane off.

## Findings and what was done

1. S3, the bound. The second was checked before each read only, so a read that took long ended
   past it: 1.36 s with reads of 400 ms, and up to the 30 s bound of one runtime call. The
   design and the changelog say "at most one second". Done: the sleep and the read run under
   the one deadline; a read still running when it passes is dropped, which kills its runtime
   call, and the state read before it stands. Case
   `a_slow_read_keeps_the_wait_within_one_second`: three reads, between 1 s and 1.05 s.
2. S3, the gate held too little. The commit's case called `oom_flag_settled` alone. Three
   mutants: the 50 ms sleep removed and the condition for the second read inverted left it
   green; the bound removed ended only at the runner's 60 s kill. Done: the four `monitor`
   cases are kept (`an_embedders_kill_is_read_once`, `a_deadlines_kill_is_read_once`,
   `an_unexplained_kill_reads_a_late_flag_and_names_memory`,
   `an_unexplained_kill_without_a_flag_is_read_every_50_ms_for_a_second`). They kill the first
   two mutants by assertion and the third at their own 20 s wait.
3. S3, a wrong comment. It called status 137 without the flag the one state a late flag can
   change; a late flag would also change what an end with another signal status names, and
   those are read once. Done: the comment says so, and so does the design.
4. S3, the lane case's message. It gave "no OOMKilled within a second" for every limit other
   than `Memory`, `WallTime` included, where no second read runs. Done: that message stands on
   a first assertion that the end names a limit; the comparison with `Memory` follows it.
5. S3, "sent" and "asked". The monitor reads the embedder's flag after the client returned, so
   a `signal(Kill)` asked when the container had ended, which sent nothing, ends the reading
   too. A flag set after it is then not read and the end names no limit, as before the commit;
   no end names `Memory` wrongly. Not shown happening. Done: the design and the code comments
   say "asked", and the design states this case.

## Not established

- The three mutants ran against `eb60fe69a`. None was run again after the change of finding 1.
- When a container runtime sets the flag relative to the exit: no container was run in this
  pass. The `oci` lane's cases run on the forge only.
- The stall's part of the condition for the second read: read, not run.
- Podman.
- That a run in its second read, which counts as running for up to one second longer, makes a
  start refused at the adapter's bound on tracked runs: read, not shown.
