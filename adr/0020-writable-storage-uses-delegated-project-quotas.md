---
status: accepted
date: 2026-08-31
---

# ADR 0020: writable storage uses delegated project quotas

## Context

The workspace is the only persistent writable mount in an exec, but its existing
`workspace.max-file-bytes` fact limits one API mutation rather than aggregate storage. File-size
rlimits and periodic directory scans are bypassable or racy, while an exec may also need bounded
ephemeral disk that is distinct from memory-backed `/tmp`.

## Decision

Optional workspace storage and per-exec `/scratch` limits contain both a byte ceiling and an inode
ceiling. The Linux host driver serves them only through a project-quota-capable workspace
filesystem and an operator-delegated project-id range. It proves enforcement and inheritance before
publishing capability facts. Otherwise requests receive named `unserved` outcomes.

Quota identity is private, durably allocated before driver dispatch and released only after bounded
cleanup proves zero use and physical absence. `/workspace` retains its lifetime and sharing
semantics; `/scratch` is private to one exec; `/tmp` stays memory-backed. Quota exhaustion and global
filesystem exhaustion remain distinct observations.

## Consequences

Concurrent processes and file API calls cannot cross a declared workspace ceiling, and empty-file
amplification is bounded. Operators must prepare a supported filesystem and delegate identifiers;
an ordinary unconfigured personal daemon continues serving unquotaed workspaces and explicitly
refuses quota requests. Product storage reservations and billing remain outside Substrate.

The complete contract and recovery rules are in
[design 17](../docs/design/17-resource-accounting-and-storage-quotas.md).
