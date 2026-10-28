---
format: aep.planning-md/3
id: story:linux-adapter-serves-scoped-workspace-write
kind: story
status: implemented
title: The linux adapter serves a scoped workspace write
tags:
- adr-0109
relations:
- decomposes: epic:confinement-contract-family
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-27T13:13:28Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-27T13:13:28Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-27T18:30:55Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1,"review_outcome":3}}}
---
## Outcome

A consumer profile names the workspace subpaths a run may write, and the `linux` adapter enforces
them: a write elsewhere in the workspace and a write over `.git` are refused.

## Decision

Session b10x-chef-orchestrator, decision D87 (2026-10-27): 0.13.0 ships from pull request 163
unchanged; this story is the next release on the follow-up line. Reason given: a consumer moving
onto the `linux` adapter would otherwise lose two protections its own adversary cases (M19, M20)
require.

## Evidence (read at a62a2960)

- `crates/substrate-host/src/confinement/linux.rs:281` sets
  `workspace_access: WorkspaceAccess::ReadWrite` for every profile.
- The module header (`linux.rs:23`) lists `sealed_paths` among the facts `linux` does not prove; a
  profile requiring it is refused `confinement.fact-unserved` today.
- The host driver already serves `substrate_wire::WorkspaceAccess::Scoped`
  (`crates/substrate-host/src/process.rs:4763`).

## Acceptance

1. A consumer profile names writable workspace subpaths through the consumer surface. The change
   is additive to the frozen surface (`tests/confinement-public-items.txt`) and the CHANGELOG
   states it.
2. On `linux`, a write outside the named subpaths and a write over `.git` are refused; one case
   each, run once.
3. `sealed_paths` is either served on `linux` or stays refused by name; the story records which.
4. Adversary pass and security review, every finding recorded with its outcome.

## Design

Changes a capability of the consumer surface: invariant 8 requires a design note or ADR before
code. The design is
[`docs/design/29-scoped-workspace-write-and-output-tail.md`](../../../docs/design/29-scoped-workspace-write-and-output-tail.md),
§§ 1–4 and 8; it needs no ADR (the note says why). `sealed_paths` stays refused on `linux` by name.
