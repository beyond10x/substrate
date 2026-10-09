---
format: aep.planning-md/3
id: epic:host-run-gates
kind: epic
status: draft
title: Run repository gates and session commands under the host driver
revision: 1
---
# Epic: run repository gates and session commands under the host driver

## Problem

A spike on 2026-10-09 ran a repository's gate and a session's commands through the in-process host
driver on a developer workstation with a systemd-delegated user scope. Four limits in this
repository stopped it or made it impractical:

1. The delegated user scope carries the `cpu`, `memory` and `pids` controllers but not `io`.
   `probe_resource_usage` (`crates/substrate-host/src/probe.rs`) requires `io.stat`, so
   `exec.resource-usage` is withheld and every exec that requests the measurement is refused
   `exec.metrics-unserved`.
2. `ProcessCgroup::create` (`crates/substrate-host/src/process.rs`) clamps `cpu.max` to one period:
   every exec runs on one core whatever it declared. A cold `cargo test` took 184.7 s wall on 1 of
   20 cores.
3. An adopted directory that is a linked Git worktree has a `.git` file pointing at the
   repository's common directory outside the workspace, so `git status` exits 128.
4. `HostDriver::open` creates `.substrate-apertures` in the workspace root on every open, even with
   no aperture declared. An embedder that adopts a managed worktree uses that worktree's parent as
   the root, which holds every other worktree.

## Outcome

The host driver serves CPU, memory and pids usage on a host without `io`, honours a configurable CPU
limit, can give a linked worktree its Git common directory, and writes no aperture state into the
workspace root unless asked to.

## Stories

- `story:exec-usage-without-io-controller`
- `story:configurable-exec-cpu-ceiling`
- `story:linked-worktree-git-common-dir`
- `story:aperture-state-outside-workspace-root`

## Specification

The ESS projection (`spec/coverage.md`) covers the accepted-operation ledger and names filesystem,
cgroup and aperture enforcement as outside it. These stories are proved by the host's native tests
and the delegated lane, as every confinement change before them.
