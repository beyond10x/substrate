---
format: aep.planning-md/3
id: story:a-backend-the-kernel-does-not-execute-is-a-refused-start
kind: story
status: draft
title: A backend the kernel does not execute is a refused start
tags:
- defect
relations:
- informed_by: review-result:cgroup-join-security-review
scope:
- confidence: inferred
  path: crates/substrate-host/src/process.rs
- confidence: inferred
  path: crates/substrate-host/src/secrets.rs
revision: 2
---
# Story: a backend the kernel does not execute is a refused start

## Outcome

Two rules of the host driver's confined start (`ProcessRuntime::start_inner`,
`crates/substrate-host/src/process.rs`) hold and each has a case:

1. A confinement backend the kernel does not execute is a refused start that carries the
   kernel's error. No run is recorded for it.
2. A spawn error does not prove that no process exists. Where the spawn answers an error, the
   exec's cgroup is killed before it is proved empty.

## Where the requirement comes from

The security review of the cgroup join
(`review-result:cgroup-join-security-review`, 2026-11-23), which states both rules for code
that change did not touch. Invariant 3: a missing guarantee is a named refusal. Stakeholder:
the operator.

## What changes

1. The forked child can still report at `execve`, after the step that closes the descriptors
   the backend must not inherit. Which way (the reporting channel kept out of the closed
   ranges, or a second report) is decided when the story is taken, and the descriptors the
   backend inherits stay what they are today.
2. The error path of the spawn kills the cgroup before `contain_cgroup` proves it empty.
3. One case per rule in an existing test binary of the host crate, delegated lane. The case
   for rule 1 was written by the review and is kept outside the repository until this story
   is taken; it runs in under 3 s.

## Acceptance

Both cases hold in the delegated lane, each in under 5 s, and the descriptor cases of
`process::tests` hold unchanged.

## Not in scope

No wire change: `exec.spawn-failed` exists.
