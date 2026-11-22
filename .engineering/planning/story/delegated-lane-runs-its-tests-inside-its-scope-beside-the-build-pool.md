---
format: aep.planning-md/3
id: story:delegated-lane-runs-its-tests-inside-its-scope-beside-the-build-pool
kind: story
status: draft
title: The delegated lane runs its tests inside its cgroup scope on a host with the build pool
summary: On the shared workstation a test started through the pool's cargo shim runs outside the lane's scope, so the lane is red there and green on the hosted runner
tags:
- defect
- gate
revision: 1
---
# Story: the delegated lane runs its tests inside its cgroup scope on a host with the build pool

## Outcome

`bash scripts/delegated-lane.sh` on the shared workstation runs each test process inside the
delegated scope the script asked for, and a test process that is outside it says so by name.

## What was seen

2026-11-22, on `main` at `2db64592c` and on a branch: the lane ended red in the ten cases of
`adversary4_exec_denied`, each "linux opened with the helper did not prove ExecSubpaths", with
`served_families: [Workspace, Audit], facts: []`. The hosted `Confinement lane` passed for the
same commit.

Cause, shown with one variable changed (same scope, same environment, same test binary): on
this host `cargo` is a shim of the build pool, and a job it starts runs in the pool daemon's
cgroup, not in the caller's. `probe_cgroup` (`crates/substrate-host/src/probe.rs`) finds the
test process outside the configured root and returns false without a reason, so every fact
that depends on it is absent. Started directly inside the scope the binary passes 10 of 10;
started through the pool it fails with the message above. Load, the kernel, bubblewrap,
AppArmor, the seccomp floor and the executable lists were each ruled out by that run.

## Where the requirement comes from

The collection's rule "The local gate equals the forge": what the workstation does differently
is written in the repository and used by the local lane. A release needs a delegated-lane
record; the hosted lane gives it, and the local lane must not read red for a reason that is not
the source. Stakeholder: the operator.

## What changes

1. The lane builds through the pool (`cargo test --no-run`, one call per package it runs) and
   then runs each `cargo test` with the pool's documented exception for one command
   (`SLOT_SHIM_OFF=1`, or `slot --local`), so the test process starts inside the scope and
   compiles nothing there. The variable means nothing on the hosted runner, so the script is
   the same in both places.
2. The `delegated()` guard of the lane's cases, or one preflight in the script, compares the
   test process's own cgroup with `SUBSTRATE_VECTORS_CGROUP_ROOT` and fails with a line that
   names both, in place of "did not prove".
3. The tests then run outside the pool, single-threaded as the lane requires. The pool's session
   answered on 2026-11-22 that no documented way keeps a pool job in the caller's cgroup, named
   the two exceptions of step 1, and filed a draft story in the pool's repository, "a job can
   run in the caller's cgroup". When that exists the lane uses it and this exception ends.

## Acceptance

On the shared workstation the lane ends green at a commit whose hosted lane is green, and a run
whose tests are started outside the scope ends with the line of step 2.
