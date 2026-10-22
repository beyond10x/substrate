---
format: aep.planning-md/3
id: review-result:adversary-w20-u1-pass-2
kind: review-result
status: active
title: Adversary pass 2, wave 2026-10-17b unit u1 (adopted-workspace runtime vectors)
relations:
- reviews: story:exec-in-an-adopted-directory
revision: 1
---
# Adversary pass 2, wave 2026-10-17b unit u1 (adopted-workspace runtime vectors)

Reviewed: the correction commit 55082a11 (exec binds the workspace by a no-symlink descriptor).
Verdict: needs change. Judged by reading; the seven cases written were not run in this pass (the
build was refused for disk).

| # | where | finding | origin |
|---|---|---|---|
| A1 | `crates/substrate-host/src/process.rs` `admit_workspace_access` | the scoped-access subtree walk runs before the pin and follows a swapped entry: a removed adopted entry is `exec.workspace-subtree-absent`, not `resource.not-found`, and a swapped link is subtree-absent, not `workspace.path-escape`; the walk reveals whether the subtree exists beneath the link target | pre-existing |
| A2 | `crates/substrate-host/src/fs.rs` `pin_workspace_directory` | the pin reopens the workspaces root by path with a following open, while the file routes use the root descriptor pinned at driver open, so a swapped root makes exec bind another object than the file routes serve | pre-existing |
| A3 | `crates/substrate-host/src/fs.rs` | the pin's doc comment sits between `validate_root_name`'s doc comment and that function | introduced |

Pass 1's F1-F5 are closed: no exec path binds a workspace by name; the descriptor is close-on-exec
and inherited by bubblewrap only; refusals are durable before dispatch; a created workspace takes
the same pin.

Cases added (host crate): seven `pass2_` cases, three expected red (A1 twice, A2 once).
