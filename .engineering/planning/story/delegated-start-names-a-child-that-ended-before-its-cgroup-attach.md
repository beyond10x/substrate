---
format: aep.planning-md/3
id: story:delegated-start-names-a-child-that-ended-before-its-cgroup-attach
kind: story
status: draft
title: A delegated start whose child ended before the cgroup attach is diagnosed
tags:
- defect
scope:
- confidence: inferred
  path: crates/substrate-host/src/process.rs
- confidence: inferred
  path: crates/substrate-host/tests/writable_root_bind.rs
revision: 3
---
# Story: a delegated start whose child ended before the cgroup attach is diagnosed

## Outcome

It is known why a delegated exec start answered `exec.cgroup-write-failed` with
`cgroup.procs: No such process` once on a hosted runner, and the start either cannot end that
way for a child that should run, or the case that saw it states the condition.

## What was seen

2026-11-23, hosted confinement lane at `d0bd8be`, attempt 1 of run 45161004811:
`a_root_swapped_for_a_symlink_after_admission_still_mounts_the_admitted_directory`
(`crates/substrate-host/tests/writable_root_bind.rs`) panicked with `the delegated lane must
dispatch ex_rw_swapped: exec.cgroup-write-failed cgroup.procs: No such process (os error 3)`.
The four other cases of the target passed, and the same job passed on the rerun. The start
failed closed with a named code; no wrong directory was mounted. The release did not change
the host driver's start path.

## What changes

First a feedback loop: the case run in a loop in a delegated scope under load until it shows
the refusal, with the child's exit status and the sandbox's stderr captured. Then the cause is
named. Two hypotheses to test, neither checked: the sandbox process ends early for a root that
is a link, and the driver writes a process id it has already reaped.

## Acceptance

The cause is written in the story with the run that showed it. If the driver can report the
child's own end, the start answers the code of that end, with a case; if the condition is the
runner's, the case states it.

## Cause, 2026-11-23

The driver's confined start (`ProcessRuntime::start_inner`,
`crates/substrate-host/src/process.rs`) named processes of the run to the kernel by process id
after the spawn. A process id names a process only while the process exists, and the
confinement backend forks processes that end within the start. Neither hypothesis of *What
changes* above was the cause: the root being a link plays no part, and the driver reaped
nothing early.

The run that showed it: a backend stand-in that forks short-lived children before it becomes
bubblewrap (`forking_backend` in `crates/substrate-host/tests/writable_root_bind.rs`), 100
starts in a delegated scope on a workstation at a load average of 33 to 40.

## What changed

The forked child joins the exec's cgroup itself, as the first thing it does after the fork and
before it executes the backend (`join_cgroup`). Every process the backend forks is born in the
cgroup, and the driver moves none and looks for none. The join descriptor is close-on-exec,
above the secret-slot range, and never a standard descriptor. A child that cannot join does
not execute, and the start is refused `exec.spawn-failed`.

With the change: 100 of 100 starts of the forking backend dispatched, 102.5 s, same host and
load. The cases:

- `a_backend_that_forks_short_lived_children_is_started_inside_its_cgroup`
  (`writable_root_bind`, delegated lane);
- `review_every_start_path_runs_its_backend_inside_the_cgroup_without_a_cgroup_descriptor`
  (the same binary; an exec, a pipe session and a pty session; 20 of 20 runs);
- `review_a_leader_its_cgroup_refuses_executes_nothing_and_the_start_leaves_nothing` (the same
  binary; 20 of 20 runs);
- `process::tests::a_child_that_cannot_join_its_cgroup_does_not_execute` and
  `process::tests::the_join_descriptor_is_above_the_slot_range_and_close_on_exec` (library,
  portable).

The acceptance's first sentence is met by the section above. Its second is met by the first
alternative, made stronger: the start does not end this way for a child that should run, so
there is no code of the child's end to answer.

`story:confined-start-survives-a-process-that-ends-before-its-cgroup-write` records the same
refusal seen on pull request 258 and is closed by the same change: its step 2 took the first
alternative (the process cannot run before it is placed).
