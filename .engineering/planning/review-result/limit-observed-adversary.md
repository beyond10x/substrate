---
format: aep.planning-md/3
id: review-result:limit-observed-adversary
kind: review-result
status: active
title: 'Adversary pass: an ended run''s observation names the limit that ended it'
relations:
- reviews: story:run-observation-names-the-limit-that-ended-it
revision: 1
---
# Adversary pass: an ended run's observation names the limit that ended it

2026-11-24, on the first commit of the branch `feat/run-observation-names-its-limit`
(`c50488cf3`): design 36, story 1. The adversary added five cases in test modules of
`crates/substrate-host/src/confinement/systemd.rs` and `crates/substrate-host/src/process.rs`
and changed no production line. Verdict: needs change. Host library selection executed 288 to
293, red 3; `ess_confinement_local` and `consumer_surface` 47 of 47. S1: 1. S2: 2. S3: 3.

The brief of the pass listed design 36 D4's order wrongly. The pass held the design's order:
output stall, egress bytes, memory, CPU time, wall time.

## Held

- `linux` names a limit only for a run that is not running, and reads it from the driver's
  record, which no adapter path discards.
- `pod` writes the limit before the phase, never resets it, and keeps it when an ended run
  becomes unknown.
- The orders of `systemd`, `oci` and `pod` are D4's.
- Each run has a cgroup of its own, and the end read comes before the cgroup is removed.
- The end reads are files the kernel writes, and no wait was added to the kill path.
- `oci_limit` and `pod_limit` take only the inputs their rows of D7 allow.
- The public items are those of D26.
- A signal of the embedder, a program that kills itself and an exit status of 137 name no limit
  on `linux`; a signal of the embedder names none on `local` and `pod`.
- Two cases, green, hold what no case held before: past both the CPU budget and the deadline
  the `systemd` watch names CPU time, and `EndRead::of` names memory before an imposed bound and
  keeps an unread count absent.

Not established by the pass: no delegated, `oci` or `pod` lane ran.

## Findings and what was done

1. S1. Rule asked (D1): a run that ended on its own names no limit. On `systemd`, a deadline
   that passes while the ended run's tree drains must record nothing. Done in the branch's
   second commit: `Leader::impose` decides under the `exited` lock and records no limit once
   the leader exited or the embedder stopped the run. Cases:
   `a_run_whose_leader_exited_before_its_deadline_names_no_limit_while_its_tree_drains` and
   `a_run_the_embedder_stopped_names_no_limit_when_its_deadline_passes_after`.
2. S2. Rule asked (D7, `linux`): a child that exited is not recorded ended by a bound that
   became due beside its exit. Done: the supervision's wait takes the child's exit first
   (`wait_for_child`), and the `oci` adapter's wait does the same. Case:
   `a_child_that_exited_before_its_deadline_is_not_recorded_timed_out`, one scenario on a pipe
   the case holds.
3. S2. Rule asked (D4): a run past both its CPU budget and its deadline is named CPU time.
   Done: the deadline's branch reads the cgroup's CPU usage. Case:
   `past_both_the_cpu_budget_and_the_deadline_the_run_is_recorded_cpu_exhausted`.
4. S3. The `oci` rule for memory was documented more narrowly than the adapter can hold: the
   runtime's state does not tell a main process that exits by itself with a status of 129 to
   192 from one that was killed. Done: the documentation of `oci_limit` and D7's `oci` note say
   so.
5. S3. Rule asked (D1, D5): a stall names a limit only where it ended the run. Done, decided
   by the session that works this repository and put to the story's stakeholder with the pull
   request: `OutputStall` is named only where the stall came
   before the leader's exit; `output_stalled` and `OutputEvent::Stalled` still report every
   stall. Cases: `a_local_run_that_exits_0_before_its_unread_output_stalls_names_no_limit`,
   `a_stall_names_the_output_stall_limit_only_where_the_leader_had_not_exited`, and for `linux`
   `a_stall_after_the_childs_exit_names_no_limit_and_one_before_names_the_stall`.
6. S3. The `oci` wait had the form of finding 2. Done with finding 2.

The pass's two cases that repeated a scenario were rewritten as one scenario each.
