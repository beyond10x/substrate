---
format: aep.planning-md/3
id: story:nested-exec-cgroups-are-removed-depth-first
kind: story
status: implemented
title: Nested cgroups under an exec are removed depth-first
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-23T08:41:49Z", actor: "agent:b10x-chef-orchestrator", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-23T08:41:49Z", actor: "agent:b10x-chef-orchestrator", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-23T14:58:55Z", actor: "agent:b10x-chef-orchestrator", revision: 4, decided_on: {"recorded":{"test_result":1,"review_outcome":1,"verification":2}}}
---
# Story: Nested cgroups under an exec are removed depth-first

## Context

Found by the 2026-10-23 wave (unit u2, security review pass 1, finding F4). `prove_empty_and_remove`
(`crates/substrate-host/src/process.rs`, used near `:1651`) kills nested cgroups through
`cgroup.kill` but never removes them depth-first, so the exec's own cgroup cannot be removed. The
entry then holds its tracking slot until restart, and the restart's reconcile refuses. A confined
child cannot create a nested group (the bubblewrap argv mounts no `/sys`, `process.rs:2449-2476`);
a process outside the sandbox with write access to the delegated subtree can.

## Acceptance

An exec cgroup holding empty nested groups is removed, nested groups first, by the retry round or
at exit; a delegated case creates nested groups from outside the sandbox and observes the exec's
cgroup gone and its slot free without a restart.
