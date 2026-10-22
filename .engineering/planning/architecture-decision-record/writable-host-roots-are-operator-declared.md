---
format: aep.planning-md/3
id: architecture-decision-record:writable-host-roots-are-operator-declared
kind: architecture-decision-record
status: accepted
title: A writable root is a named host path or a Git checkout
relations:
- decides: story:writable-host-roots
- decides: story:writable-roots-wire-0-22-0
- decides: story:writable-git-roots
revision: 5
transitions:
- {from: "proposed", to: "accepted", at: "2026-10-22T10:47:48Z", actor: "agent:b10x-chef-orchestrator", revision: 5}
---
## Status

Accepted 2026-10-22 as
[`adr/0038-writable-roots-are-named-host-paths-or-git-checkouts.md`](../../../adr/0038-writable-roots-are-named-host-paths-or-git-checkouts.md),
which carries the full decision. It amends ADR 0023 § Consequences, ADR 0010 and ADR 0033.

## Decision, in short

The operator, 2026-10-22, verbatim: "either name physical paths, or provide git source".

1. Kind 1: `{"host": "<name>", "mount": …}` selects a directory the operator declared physically
   with `--writable-root <name>=<path>`; opened with `openat2` `RESOLVE_NO_SYMLINKS`, owner-checked,
   mounted `--bind-fd`. Fact `exec.writable-roots.host`.
2. Kind 2: `{"workspace": "<ws_…>", "mount": …}` names a Ready Git-source workspace of the same
   subject (design 21), mounted writable with its `.git`. Fact `exec.writable-roots.git`.
3. One mount rule set, at most 8 entries, applied record per root, `workdir` may lie under one,
   successor bundle `substrate-wire/0.22.0`.

## Options replaced

Draft B (revision 2 of this artifact) proposed kind 1 alone. Options A (more adopted workspaces)
and C (none) are not taken; the operator's answer replaced all three.
