---
format: aep.planning-md/3
id: architecture-decision-record:writable-host-roots-are-operator-declared
kind: architecture-decision-record
status: proposed
title: Writable host roots are operator-declared and bound by descriptor
relations:
- decides: story:writable-host-roots
revision: 2
---
## Status

Proposed 2026-10-22, not accepted. No implementation until the operator accepts it. When accepted
it becomes `adr/0038-writable-host-roots-are-operator-declared.md` and amends the sentence in ADR
0023 § Consequences ("writable paths outside the adopted workspace are still impossible") and the
matching sentences in ADR 0010 and ADR 0033.

## Context

A confined child can write only `/workspace` (and `/scratch` when set). takt's harness session must
write its working directory, decision channel and config home under the user's home, and git must
write a linked worktree's common git directory in the primary checkout. ADR 0035 adoption cannot
serve these: it admits only a single-component root name under the daemon's workspace root
(`adr/0035-an-operators-directory-is-served-after-an-explicit-adopt.md`, decision 1).

## Decision

1. A **writable host root** is a named operator declaration in daemon configuration
   (`--writable-root <name>=<absolute host path>`), as an egress aperture is (ADR 0013). A request
   selects one by name and never carries a host path for it. Configuration owns which host
   directories are writable; a request selects among them.
2. The start field is `writable_roots: [{name, mount}]`. `mount` is absolute and normal; it may
   equal the declared host path, so host paths resolve inside the sandbox. It may not be or lie
   under `/usr`, `/bin`, `/lib`, `/lib64`, `/proc`, `/dev`, `/tmp`, `/workspace`, `/runtime`,
   `/scratch` or `/etc`, nor collide with another root or a read-only root.
3. The daemon opens each declared directory once per start with `openat2` `RESOLVE_NO_SYMLINKS`,
   `O_PATH | O_DIRECTORY`, and bubblewrap mounts it `--bind-fd`, so a rename after admission cannot
   redirect it. The directory must be owned by the daemon's uid.
4. A capability fact `exec.writable-roots` lists the declared names, published only after a
   throwaway sandbox proves a bound root is writable and its parent is not. Absent fact or
   undeclared name: `unserved` (501), naming the root. Never widened, never dropped silently.
5. The applied confinement lists every writable root: name, host path, mount.
6. Successor bundle, additive; `workdir` (design 26) may then also lie under a writable root.

## Consequences

Substrate gains host write authority outside the workspace for the first time. It is bounded by
operator declaration, name reference, descriptor binding, an owner check and an observed record.
No quota applies to a writable root: it is the operator's directory, as an adopted workspace is.

## Options not taken

- A: mount more adopted workspaces writably. Cannot reach a directory outside the workspace root.
- C: none; Loom confines only `aep` and git where `.git` is inside the workspace.

Orchestrator, 2026-10-22: "Accepted: draft B (proposed, not accepted, no implementation) ... I put B
to Timo as the recommendation."
