---
format: aep.planning-md/1
id: story:exec-oom-kills-the-whole-tree
kind: story
status: draft
title: An exec OOM kills the whole cgroup and is named on the observation regardless of measurements
summary: No memory.oom.group=1; exec.memory-limit recorded only when measurements requested (process.rs:2848-2860).
tags:
- review
relations:
- decomposes: epic:review-2026-09-03-findings
scope:
- confidence: cited
  path: crates/substrate-host/src/probe.rs
- confidence: cited
  path: crates/substrate-host/src/process.rs
revision: 6
---
# Story: An exec OOM kills the whole tree

## Context

`Cgroup::create` writes `pids.max`, `memory.max`, `memory.swap.max=0` and `cpu.max`
(`crates/substrate-host/src/process.rs:2848-2860`) and not `memory.oom.group=1`. The kernel then
kills one process on OOM and the rest of the tree continues, which is not the whole-tree semantics
the safety envelope names (`AGENTS.md` § Safety envelope). The `exec.memory-limit` refusal is
derived from `memory_oom_kills` in the terminal usage, which exists only when the client asked for
`measurements` (`process.rs`, `run_child` and `record_resource_bound`), so an OOM on a run without
measurements is reported as an ordinary exit. `cpu.max` is clamped to one CPU
(`process.rs:2858`), which is a documented choice worth stating on the capability fact.

## Acceptance

An exec whose child exceeds `memory.max` ends with no process left in its cgroup, observed in the delegated lane.

## Notes

`memory.oom.group` is a cgroup v2 file present on every kernel the probe already requires; probe it and withhold the exec facts where it is absent. Naming `exec.memory-limit` on the observation when `measurements` was not requested is a second change on `record_resource_bound`, delivered in this story and checked by its own case rather than by the acceptance above. The one-CPU `cpu.max` clamp (`process.rs:2858`) is not changed here; stating it on the capability fact is a contract addition and, if wanted, is its own story under a successor bundle.

## Parallel work

This story shares `crates/substrate-host/src/probe.rs` with story:backend-recheck-hashes-only-on-change, story:confined-processes-cannot-nest-user-namespaces, story:daemon-image-serves-exec-or-says-it-cannot, story:exec-oom-kills-the-whole-tree and story:seccomp-denies-af-vsock; three of them also share `crates/substrate-host/src/process.rs`. Work them one at a time, or in one wave by one implementor; `aep artifact waves` sequences them.
