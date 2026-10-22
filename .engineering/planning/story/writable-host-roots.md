---
format: aep.planning-md/3
id: story:writable-host-roots
kind: story
status: draft
title: A confined child writes a host directory the operator named physically
relations:
- depends_on: story:writable-roots-wire-0-22-0
scope:
- confidence: cited
  path: crates/substrate-daemon/src
- confidence: cited
  path: crates/substrate-daemon/tests/runtime_vectors.rs
- confidence: cited
  path: crates/substrate-host/src
revision: 5
---
# A confined child writes a host directory the operator named physically

takt's harness session needs to write its working directory, decision channel and config home,
and git needs to write a linked worktree's common git directory, all outside the workspace
(takt `dependency-blocker:loom-child-confinement`, G5). Kind 1 of
`adr/0038-writable-roots-are-named-host-paths-or-git-checkouts.md` serves it.

## Outcome

`--writable-root <name>=<absolute host path>` declares a directory. A start's
`{"host": "<name>", "mount": "<path>"}` entry mounts it writable at `mount`: opened once with
`openat2` `RESOLVE_NO_SYMLINKS`, `O_PATH | O_DIRECTORY`, owner-checked against the daemon uid on
that descriptor, bound with `--bind-fd`. The applied record lists name, host path and mount.

## Acceptance

- Startup refuses a declaration that is relative, not normal, missing, not a directory, not owned
  by the daemon uid, or under or containing the workspace root or state directory, naming it.
- `exec.writable-roots.host` is published only after a throwaway sandbox proves the bound root
  writable and its parent read-only; otherwise a host entry is refused `unserved` (501).
- An undeclared name is refused `unserved` with the name; each mount-rule violation of ADR 0038
  decision 4 is a named 422 before dispatch.
- Delegated vectors: a child writes a file in the root and it appears on the host; the parent and a
  sibling stay unwritable; a symlink swapped in after admission does not redirect the mount;
  `workdir` under the root is admitted.

## Scope

`crates/substrate-daemon` (configuration, admission, fact probe), `crates/substrate-host`
(`--bind-fd` mount, probe), runtime vectors. Depends on the `0.22.0` wire story.
