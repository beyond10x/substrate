---
format: aep.planning-md/3
id: story:confined-start-survives-a-process-that-ends-before-its-cgroup-write
kind: story
status: draft
title: A confined start does not fail when its process ends before it is moved into its cgroup
summary: One hosted confinement-lane run answered exec.cgroup-write-failed with ESRCH for a run that ended at once
tags:
- defect
- flaky
scope:
- confidence: cited
  path: crates/substrate-host/src/process.rs
- confidence: cited
  path: crates/substrate-host/tests/writable_root_bind.rs
revision: 3
---
# Story: a confined start does not fail when its process ends before it is moved into its cgroup

## Outcome

`start_exec` on the host driver does not answer `exec.cgroup-write-failed` because the process it
started was already gone when the driver wrote `cgroup.procs`.

## What was seen

Pull request 258, head `dae5c8b2e`, hosted `Confinement lane`: the case
`a_root_swapped_for_a_symlink_after_admission_still_mounts_the_admitted_directory` in
`crates/substrate-host/tests/read_only_root_bind.rs` panicked with "the delegated lane must
dispatch the swapped-root exec: exec.cgroup-write-failed cgroup.procs: No such process (os error
3)". The change under test touched neither the case nor the driver. The rerun of the same commit
passed. Seen once.

## Read, not run

`crates/substrate-host/src/process.rs` raises the code where a write to a cgroup file fails. The
case starts `/bin/cat` on a small file through a backend script that swaps a directory first. A
process that has ended before the driver writes its id answers `ESRCH`. Whether the driver is
meant to place the process before it can run, and why it did not here, was not read.

## Where the requirement comes from

The collection's rule "The local gate equals the forge". Beyond the lane: a run that ends at once
is an ordinary run, and its caller should get its exit status, not a failed start. Stakeholder:
the operator.

## What changes

1. Read how the driver orders the start against the write of `cgroup.procs`, and state it here.
2. Either the process cannot run before it is placed, or `ESRCH` on that write for a process the
   driver started is read as "ended" and the run's outcome is observed as usual. Which one
   follows from step 1; the limits of a run must hold from its first instruction, so the first is
   the likely answer.
3. A case that starts a program which ends at once, without a retry in the lane.

## Acceptance

The case of step 3 holds, and the lane's case above has not failed this way in ten release runs.

## Closed by the change of the sibling story, 2026-11-23

Step 1: the driver wrote the process id of the spawned backend, and of each process it found
beneath it, to `cgroup.procs` after the spawn. Step 2 took the first alternative: the forked
child joins the cgroup itself before it executes the backend, so the process cannot run before
it is placed (`join_cgroup`, `crates/substrate-host/src/process.rs`). Step 3:
`a_backend_that_forks_short_lived_children_is_started_inside_its_cgroup` in
`crates/substrate-host/tests/writable_root_bind.rs`, with no retry; 100 of 100 starts in a loop
on a workstation. The cause, the run and the other cases are in
`story:delegated-start-names-a-child-that-ended-before-its-cgroup-attach`.

The second half of the acceptance (ten release runs of the lane without this refusal) is open
and is read from the hosted `Release lanes` runs on `main` after the change.
