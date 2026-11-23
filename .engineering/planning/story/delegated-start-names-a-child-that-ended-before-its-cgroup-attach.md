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
revision: 2
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
