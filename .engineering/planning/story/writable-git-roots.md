---
format: aep.planning-md/3
id: story:writable-git-roots
kind: story
status: implemented
title: A confined child writes a Git checkout mounted outside /workspace
relations:
- depends_on: story:writable-roots-wire-0-22-0
scope:
- confidence: cited
  path: crates/substrate-daemon/src
- confidence: cited
  path: crates/substrate-daemon/tests/runtime_vectors.rs
- confidence: cited
  path: crates/substrate-host/src
- confidence: cited
  path: crates/substrate-store/src
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-22T11:02:42Z", actor: "agent:b10x-chef-orchestrator", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-22T11:02:42Z", actor: "agent:b10x-chef-orchestrator", revision: 4}
- {from: "active", to: "implemented", at: "2026-10-22T21:51:32Z", actor: "agent:b10x-chef-orchestrator", revision: 5, decided_on: {"recorded":{"test_result":1,"review_outcome":2}}}
---
# A confined child writes a Git checkout mounted outside /workspace

Kind 2 of `adr/0038-writable-roots-are-named-host-paths-or-git-checkouts.md`: the run receives a
Git source as a writable checkout with its own `.git`, so `git` needs no common directory in a
primary checkout on the host.

## Outcome

A start's `{"workspace": "<ws_…>", "mount": "<path>"}` entry mounts a second workspace, created from
a Git source (design 21), writable at `mount`, `.git` included. It is admitted like the start's own
workspace: same subject, Ready, lease freeze and storage quota unchanged, bound by descriptor. The
applied record lists the workspace id, host path and mount.

## Acceptance

- An empty or adopted workspace, another subject's workspace, a workspace that is not Ready, and the
  start's own `workspace` are each refused by name before dispatch.
- `exec.writable-roots.git` is published only when `workspace.git` is and the writable-bind probe
  passes; otherwise a workspace entry is refused `unserved` (501).
- Delegated vectors: a child commits in the mounted checkout and the commit is visible through the
  workspace's Git change-set observation; the checkout's storage quota still applies.

## Scope

`crates/substrate-daemon` (admission of the second workspace), `crates/substrate-store`
(workspace admission), `crates/substrate-host` (bind), runtime vectors. Depends on the `0.22.0` wire
story. Shares the admission and bind code paths with the host-root story; run them in sequence or in
one worktree.
