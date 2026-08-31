---
status: accepted
date: 2026-09-01
---

# ADR 0023: workspace write access is explicit

## Context

The workspace sandbox currently binds all of `/workspace` read-write. A caller that needs one
writable output directory cannot ask for that smaller authority: it must either grant the whole
tree or decline execution. A downstream process envelope therefore cannot prove that its declared
writable subtrees are the only writable workspace paths.

## Decision

Every execution may select one workspace access mode: the existing read-write mode, read-only, or
scoped write access naming normalized workspace-relative directories. An omitted mode retains the
existing read-write behavior, so frozen request bytes and existing clients do not change.

Scoped paths are directories already present beneath the adopted workspace. Absolute paths,
empty or dot components, parent traversal, aliases, overlaps and any symlink component are refused
before a process is started. The host mounts the workspace read-only first and re-binds only the
admitted directories read-write. It opens the root and every scoped directory component without
following links, and bubblewrap mounts those inherited directory descriptors rather than resolving
the path again, so a concurrent rename cannot redirect the grant. Applied confinement reports the
exact selected mode and paths.

The capability is published only after a throwaway sandbox proves that an admitted subtree is
writable while its parent and a sibling are not. A host that cannot prove that property leaves the
fact absent and refuses read-only or scoped requests by name; it never widens them to read-write.

## Consequences

Existing callers keep their current workspace access. New callers can request least-authority
write scopes and compare the request with the applied record. The request remains workspace policy,
not a general host bind: writable paths outside the adopted workspace are still impossible.
