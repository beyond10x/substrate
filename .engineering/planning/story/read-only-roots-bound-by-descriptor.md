---
format: aep.planning-md/3
id: story:read-only-roots-bound-by-descriptor
kind: story
status: active
title: Read-only roots are bound by descriptor, not by path
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-22T04:37:18Z", actor: "agent:b10x-chef-orchestrator", revision: 3, executor: "agent:substrate-wave-20261022", correlation: "wave-2026-10-22-confinement-gaps"}
- {from: "proposed", to: "active", at: "2026-10-22T04:37:18Z", actor: "agent:b10x-chef-orchestrator", revision: 4, executor: "agent:substrate-wave-20261022", correlation: "wave-2026-10-22-confinement-gaps"}
---
# Read-only roots are bound by descriptor

`read_only_roots` are mounted with `--ro-bind <host_path> <mount>`
(`crates/substrate-host/src/process.rs`, the loop after the capsule bind): bubblewrap resolves the
path again after admission checked it with `symlink_metadata`, so a rename or symlink swap in that
window mounts another directory. The workspace (ADR 0023) and single files (ADR 0033) are already
bound through descriptors. Design `docs/design/26-working-directory-and-several-apertures.md` § 3.
No wire byte changes.

## Acceptance

- Each root is opened once `O_PATH | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC` via `openat2`
  `RESOLVE_NO_SYMLINKS` at admission and mounted with `--ro-bind-fd`; argv carries no root host path.
- A test that swaps a root for a symlink to another directory after admission and before launch
  shows the child sees the original directory's contents.
- A root with a symlink at any component is `exec.read-only-root-not-a-directory`; a missing one
  `exec.read-only-root-absent`.
- Every existing read-only-root case, portable and delegated, still passes.

## Scope

`crates/substrate-host/src/process.rs` (`admit_read_only_roots`, the bubblewrap argv builder), the
`fs` helpers that already open the workspace by descriptor.
