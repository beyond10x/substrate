---
format: aep.planning-md/3
id: story:session-start-publishes-exec-fields
kind: story
status: implemented
title: Session start publishes the exec fields exec start does
relations:
- decomposes: epic:host-run-gates
scope:
- confidence: inferred
  path: .github/workflows/release.yml
- confidence: inferred
  path: AGENTS.md
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: contracts/substrate-wire/0.23.0
- confidence: inferred
  path: crates/b10x-substrate-sdk/src/lib.rs
- confidence: cited
  path: crates/substrate-daemon/tests/runtime_vectors.rs
- confidence: cited
  path: crates/substrate-wire/src/lib.rs
- confidence: inferred
  path: scripts/gate.sh
- confidence: inferred
  path: website/docs/reference/contract.md
- confidence: inferred
  path: xtask/bundle-source/0.23.0
- confidence: inferred
  path: xtask/src/bundle.rs
- confidence: inferred
  path: xtask/tests/release_workflow.rs
revision: 7
transitions:
- {from: "draft", to: "proposed", at: "2026-10-23T10:35:48Z", actor: "agent:b10x-chef-orchestrator", revision: 5}
- {from: "proposed", to: "active", at: "2026-10-23T10:35:48Z", actor: "agent:b10x-chef-orchestrator", revision: 6}
- {from: "active", to: "implemented", at: "2026-10-23T14:58:56Z", actor: "agent:b10x-chef-orchestrator", revision: 7, decided_on: {"recorded":{"test_result":1,"review_outcome":1,"verification":2}}}
---
# Story: session start publishes the exec fields exec start does

## Problem

The `pipe-session-start` input schema's `exec` object, through bundle `0.18.0`, lacks `scratch`,
`measurements` and `workspace_access`, which the exec-start schema carries and the wire crate's
start input accepts on both routes. A bundle-only consumer cannot request them on a session, or
cannot tell whether a session honours them. Found while adding `read_only_files` to both start
schemas in `0.18.0`.

## Contract

A successor bundle states the three fields on the session-start `exec` object exactly as on exec
start, or, for any field a session does not serve, the daemon refuses it with a named code and the
bundle says so. Which of the two holds per field is read from the daemon first.

## Acceptance

- For each of `scratch`, `measurements` and `workspace_access`, the session-start schema and the
  daemon agree: accepted and honoured, or refused before dispatch with a named, published code.
- A runtime vector exercises each field on the session route.

## Scope

Derived 2026-10-23 by `story-scoper`. Each line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** successor bundle `0.23.0` — inferred: in `0.22.0`, `pipe-session-start.json` lacks `scratch`, `measurements` and `workspace_access`, which `exec-start.json` carries
- **Files:** `xtask/bundle-source/0.22.0/documents/schemas/inputs/pipe-session-start.json:32`, `crates/substrate-wire/src/lib.rs:18`, `:2719`, `crates/substrate-daemon/tests/runtime_vectors.rs` — cited
- **Also likely:** `xtask/src/bundle.rs`, `scripts/gate.sh`, `crates/b10x-substrate-sdk/src/lib.rs`, `.github/workflows/release.yml`, `xtask/tests/release_workflow.rs`, the per-test bundle pins, `CHANGELOG.md`, `AGENTS.md`, `website/docs/reference/contract.md`; `crates/substrate-daemon/src/app/operations.rs:802` and `crates/substrate-host/src/process.rs:767` only if a field is refused today — inferred
- **Confidence:** medium
- **Safety fact (unproven):** a session's exec reaches `ProcessRuntime::start_inner` (`process.rs:850`), which runs `prepare_workspace_access` (`:872`) and `prepare_scratch` (`:920`); the daemon seeds usage from `exec.measurements` (`crates/substrate-daemon/src/app/sessions.rs:375`).
- **Caveat:** no ADR or design document speaks to sessions and these fields; ADRs 0020, 0021 and 0023 define the fields.
