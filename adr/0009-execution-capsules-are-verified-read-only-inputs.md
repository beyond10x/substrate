---
status: accepted
date: 2026-08-14
---

# ADR 0009: execution capsules are verified read-only inputs

## Context

The raw-pipe contract can confine an argv inside a mutable workspace and a read-only host system,
but it cannot prove that a caller-declared harness digest names the bytes actually executed. A host
path would introduce mutable provisioning state and path races. Placing configuration or hook
handlers in the workspace would let the process change its own control inputs.

## Decision

Add an optional, bounded execution capsule to exec and raw-pipe start. A capsule contains a
canonical entrypoint and a sorted, unique list of regular files. Each file has a canonical relative
path, a closed role, an executable bit, exact bytes, and a SHA-256 digest. A domain-separated,
length-framed canonical algorithm hashes the manifest metadata into the capsule identity.

Before dispatch, the host driver validates all bounds and identities, decodes and hashes every
file, verifies the manifest digest, and materializes the files in a new private directory. It
creates no symlinks or special files. Bubblewrap mounts that directory read-only at `/runtime` and
the workspace separately read-write at `/workspace`. The start argv must execute the declared
`/runtime` entrypoint. The private directory remains owned by the process observation until cgroup
terminal reconciliation, then is removed by descriptor-owned temporary-directory cleanup. After a
daemon restart, orphan cgroups are reconciled before bounded startup cleanup removes stale private
capsule directories. Unexpected names, non-directories, or symlinks fail startup rather than
broadening the deletion target.

The applied confinement observation includes the exact capsule digest, entrypoint, file count,
total decoded bytes, and fixed mount point. The capability snapshot publishes the served inline
file/byte bounds. Invalid bytes, paths, ordering, digest drift, an entrypoint mismatch, unavailable
materialization, or cleanup ambiguity refuses or makes the dispatch outcome explicit; execution
never falls back to an unbound host path.

This first transport is for small development fixtures. It binds the application payload,
configuration, protocol sidecars, and hook handlers, not the host kernel, interpreter, libraries,
or read-only base system. Stable and live consumers still require signed artifact distribution and
a separately defined complete runtime closure. Capsules never carry secrets or network authority.

## Consequences

- Substrate can independently report which immutable application bytes it mounted and executed.
- Mutable workspace content cannot replace capsule control files during a run.
- The wire adds bounded request data and deterministic vectors without adding Agent, vendor, or
  product semantics.
- Capsule materialization and deletion join process-tree reconciliation and must be tested for
  tamper, digest drift, path escape, read-only enforcement, and cleanup.
- Existing non-capsule exec remains available for its existing development consumers, but it
  cannot satisfy the new capsule capability or a governed Agent profile that requires it.
