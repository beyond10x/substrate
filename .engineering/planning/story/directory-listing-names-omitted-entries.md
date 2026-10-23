---
format: aep.planning-md/3
id: story:directory-listing-names-omitted-entries
kind: story
status: implemented
title: Directory listings name the entries they omit
relations:
- decomposes: epic:review-2026-09-03-findings
scope:
- confidence: inferred
  path: .github/workflows/release.yml
- confidence: inferred
  path: AGENTS.md
- confidence: inferred
  path: CHANGELOG.md
- confidence: cited
  path: contracts/substrate-wire/0.23.0
- confidence: inferred
  path: crates/b10x-substrate-sdk/src/lib.rs
- confidence: cited
  path: crates/substrate-daemon/tests/runtime_vectors.rs
- confidence: inferred
  path: crates/substrate-daemon/tests/tls_listener.rs
- confidence: cited
  path: crates/substrate-host/src/fs.rs
- confidence: cited
  path: crates/substrate-wire/src/lib.rs
- confidence: inferred
  path: scripts/gate.sh
- confidence: inferred
  path: website/docs/reference/contract.md
- confidence: cited
  path: xtask/bundle-source/0.23.0
- confidence: inferred
  path: xtask/src/bundle.rs
revision: 7
transitions:
- {from: "draft", to: "proposed", at: "2026-10-23T12:02:50Z", actor: "agent:b10x-chef-orchestrator", revision: 5}
- {from: "proposed", to: "active", at: "2026-10-23T12:02:50Z", actor: "agent:b10x-chef-orchestrator", revision: 6}
- {from: "active", to: "implemented", at: "2026-10-23T14:58:54Z", actor: "agent:b10x-chef-orchestrator", revision: 7, decided_on: {"recorded":{"test_result":1,"review_outcome":1,"verification":2}}}
---
# Story: Directory listings name the entries they omit

## Context

story:directory-listing-survives-special-entries makes the v1 directory page and the v2 tree omit
entries the wire cannot represent (FIFOs, sockets, devices, non-UTF-8 names) instead of refusing.
Without a successor bundle it can only signal that on v2, by reusing `truncated`, and on v1 not at
all; it records both as accepted trade-offs against invariant 3.

## Acceptance

A successor wire bundle publishes a machine-readable omission signal on both the v1 directory page
and the v2 tree, distinct from `truncated`, and a runtime vector shows a directory with a FIFO
answering it on both routes.

## Notes

Needs an ADR or design document before code (invariant 8) and a successor bundle (invariant 6).
Left draft until the next bundle is cut.

## Scope

Derived 2026-10-23 by `story-scoper`. Each line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** `crates/substrate-wire` and a successor bundle `0.23.0` (`xtask/bundle-source/0.23.0/` rendered to `contracts/substrate-wire/0.23.0/`) — cited (story: "successor wire bundle"; invariant 6)
- **Files:** `crates/substrate-wire/src/lib.rs:1159` (`DirectoryPage`), `:1209` (`WorkspaceTree`), `:18` (`ADVERTISED_CONTRACT_BUNDLE`) — cited
- **Files:** `crates/substrate-host/src/fs.rs:245`, `:349-352` (`truncated: walk.limit_reached || walk.omitted`), `:785` `list_directory`, `:812` `walk_tree` — cited
- **Files:** `crates/substrate-daemon/tests/runtime_vectors.rs:46` and the case lists — cited for the pin, inferred for the new FIFO case
- **Also likely:** the bundle pin set `0.22.0` moved (`xtask/src/bundle.rs`, `scripts/gate.sh`, `.github/workflows/release.yml`, `crates/substrate-daemon/tests/tls_listener.rs`, `crates/b10x-substrate-sdk/src/lib.rs`), `website/docs/reference/contract.md`, `CHANGELOG.md`, `AGENTS.md` — inferred
- **Documents:** a new ADR or design document before code (story Notes) — cited
- **Confidence:** medium — field shape (omitted count vs a fourth entry kind) is undecided
- **Would collide with:** any unit cutting a successor bundle or moving the advertised contract pin; any unit on `substrate-host/src/fs.rs` listing code
- **Safety fact:** `DirectoryPage` and `WorkspaceTree` are `#[serde(deny_unknown_fields)]` (`crates/substrate-wire/src/lib.rs:1157`, `:1207`); a new field breaks clients pinned to an older wire crate unless the compatibility block handles it — level 2, unproven

implementable-here: no. No ADR or design document exists (invariant 8), and the signal shape is an open operator decision.
wire-change: yes
