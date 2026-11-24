---
format: aep.planning-md/3
id: review-result:cgroup-join-security-review
kind: review-result
status: active
title: Security review of the exec leader's cgroup join
relations:
- reviews: story:delegated-start-names-a-child-that-ended-before-its-cgroup-attach
- reviews: story:confined-start-survives-a-process-that-ends-before-its-cgroup-write
revision: 1
---
# Security review: the exec's leader joins its cgroup before it executes the backend

2026-11-23, an independent reviewer on the uncommitted change of
`story:delegated-start-names-a-child-that-ended-before-its-cgroup-attach`, base `f320866be`.
The reviewer did not edit the implementation. Verdict: nothing found that the change
introduces and that weakens confinement.

## What was checked, and holds

1. One spawn per start (`ProcessRuntime::start_inner`), and the join is the first statement of
   its only `pre_exec` closure. Measured for an exec, a pipe session and a pty session: the
   backend's first instruction and the confined program are inside `substrate-<id>`.
2. A join the kernel refuses executes nothing: `exec.spawn-failed`, no backend stamp, the
   cgroup removed, the next start exits 0.
3. Neither the backend nor the program holds a descriptor on a cgroup file. The join
   descriptor is close-on-exec, is in neither the inherited nor the retained set, and is
   closed with the others before `execve`.
4. Limits are set when the cgroup is created, before the fork. Timeout, drain and kill paths
   are unchanged.
5. The join is one `write` and allocates nothing, and it stands before the step that closes
   the standard library's error channel.
6. The removed code `exec.process-tree-failed` is named nowhere in the wire crate, the daemon,
   `spec/`, `contracts/`, `xtask/` or `website/`.
7. Aperture forwarders are forked by the daemon after the leader and join the cgroup
   themselves; they are unchanged.

## What changes in charging

Membership starts before `execve`. The backend's own start is charged to the run: 262144 to
565248 bytes measured for join-then-execute, against an admitted minimum memory bound of
1 MiB. Bubblewrap's first clone counts against `pids.max`. No start was shown to fail for it.

## Findings and what was done

- The author's cases never refused a join on the real start path. Done: the reviewer's case
  for it is in `writable_root_bind`
  (`review_a_leader_its_cgroup_refuses_executes_nothing_and_the_start_leaves_nothing`), beside
  the case for the three start paths.
- A join descriptor at 0, 1 or 2 would be written to as a standard stream. Done:
  `Cgroup::open_procs` hands back a descriptor above the secret-slot range or refuses the
  start, held by `the_join_descriptor_is_above_the_slot_range_and_close_on_exec`.
- The refusal's message named bubblewrap for a cgroup's refusal. Done: it names the three
  steps the child's one error number can come from.
- The changelog said "every process of the run". Done: "every process the backend forks", with
  the charge and the two codes.
- Three design documents (10, 10a, 11) describe the start as it was designed. Left: they are
  records of their decisions.
- Two rules the review states for code this change did not touch are
  `story:a-backend-the-kernel-does-not-execute-is-a-refused-start`.

## Runs

Three cases added by the review, each run alone in a delegated scope; the two kept ran 20 of
20 each. The change's own binary: 6 of 6 cases. Library case: 1 of 1.
