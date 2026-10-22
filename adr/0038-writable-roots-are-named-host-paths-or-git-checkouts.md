---
status: accepted
date: 2026-10-22
---

# ADR 0038: a writable root is a named host path or a Git checkout

Amends [ADR 0023](0023-workspace-write-access-is-explicit.md) § Consequences,
[ADR 0010](0010-declared-host-roots-are-mounted-read-only.md) and
[ADR 0033](0033-a-single-host-file-is-admitted-read-only.md): `/workspace` and `/scratch` are no
longer the only writable paths in a sandbox. Each new writable path is one of the two kinds below
and nothing else.

## Context

A confined child can write `/workspace` and, when requested, `/scratch`. Nothing else. A harness
session run under confinement needs more: its working directory, a decision channel and its
configuration home live under the user's home directory, and `git` in a linked worktree writes the
common Git directory inside the primary checkout. Each is outside the workspace root.

ADR 0035 adoption does not reach them. It admits a single-component root name directly under the
daemon's workspace root (decision 1), so a directory elsewhere on the host cannot be adopted.

Three options were put to the operator on 2026-10-22: mount more adopted workspaces writably (A),
operator-declared writable host roots selected by name and bound by descriptor (B), or no writable
path outside the workspace (C). The operator answered, verbatim: "either name physical paths, or
provide git source". That answer replaces A, B and C with the two kinds this ADR decides. B's named,
descriptor-bound selection is kind 1.

## Decision

1. **A start may carry `writable_roots`, a list of writable mounts outside `/workspace`.** Each entry
   is exactly one of two kinds, a closed union. A request never carries a host path.

2. **Kind 1, a named host path.** The operator declares it physically in daemon configuration:
   `--writable-root <name>=<absolute host path>`, as an egress aperture is declared (ADR 0013). The
   entry is `{"host": "<name>", "mount": "<absolute sandbox path>"}`.
   - The name has the root-name character set (ADR 0035 decision 1).
   - At startup the declared path must be absolute and normal, must exist as a directory owned by
     the daemon's uid, and must not lie under or contain the workspace root or the state directory.
     A declaration that fails stops startup with the name and the failed condition.
   - At each start the daemon opens the declared path once with `openat2`
     `RESOLVE_NO_SYMLINKS`, `O_PATH | O_DIRECTORY`, checks the owner on that descriptor, and
     bubblewrap mounts it with `--bind-fd`. A rename or symlink swap after admission cannot
     redirect the mount, as for read-only roots since 0.9.0.

3. **Kind 2, a Git checkout.** The entry is `{"workspace": "<workspace-id>", "mount": "<absolute
   sandbox path>"}` and names a second workspace that was created from a Git source (design 21,
   `workspace.git`). The run writes that checkout, `.git` included, at `mount`.
   - The workspace must belong to the caller's subject, be Ready, and have a Git origin. An empty or
     adopted workspace is refused; so is the start's own `workspace`.
   - Admission treats it as it treats the start's workspace: lease freeze, write authority and
     storage quota apply unchanged. It is bound by descriptor, as `/workspace` is (ADR 0023).
   - The checkout is materialized by the existing Git-source path, under its HTTPS source
     configuration, transient authority and limits. This ADR adds no new fetch path.

4. **Mount rules, both kinds.** `mount` is absolute and normal. It must not be `/`, and must not
   be, lie under or contain `/usr`, `/bin`, `/lib`, `/lib64`, `/proc`, `/dev`, `/tmp`, `/etc`,
   `/workspace`, `/runtime` or `/scratch`. It must not equal, contain or lie under another writable
   root, a read-only root or a read-only file. `mount` may equal the host path of a kind-1 root, so
   host paths written into configuration resolve the same inside the sandbox. At most 8 entries.

5. **Capability facts.** `exec.writable-roots.host` lists the declared kind-1 names. It is published
   only after a throwaway sandbox proves a root bound by descriptor is writable while its parent is
   not. `exec.writable-roots.git` is published when `workspace.git` is and the same probe passes.
   A start that names a kind whose fact is absent, or an undeclared name, is refused `unserved`
   (501) with the name. Nothing is widened, dropped or silently skipped (invariant 3).

6. **Observed record.** The applied confinement lists every writable root with its kind, its name or
   workspace id, the resolved host path, and its mount.

7. **`workdir` may lie at or under a writable root** (design 26 § 1).

8. **Successor bundle `substrate-wire/0.22.0`, additive.** It adds `writable_roots`, the two facts,
   the record members and the named refusals, each bound to a `substrate_wire` constant. Earlier
   bundles are unchanged.

## Consequences

Substrate gains host write authority outside the workspace for the first time. Two things bound it.
Kind 1 writes only directories the operator named physically, owned by the daemon's uid. Kind 2
writes only a checkout Substrate materialized itself from a configured Git source.

No quota applies to a kind-1 root: it is the operator's directory, as an adopted workspace is. A
kind-2 checkout keeps its workspace's storage quota.

A linked Git worktree whose common directory sits in a primary checkout can be served two ways:
the operator declares the primary checkout as a kind-1 root, or the run receives its own kind-2
checkout, which has its own `.git`.

The isolation floor is unchanged: no network unless an aperture is declared, no nested user
namespace, read-only host roots, cleared environment.
