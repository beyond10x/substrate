---
format: aep.planning-md/3
id: story:bundle-publishes-read-only-roots
kind: story
status: implemented
title: The published bundle states read-only roots
relations:
- decomposes: epic:host-run-gates
scope:
- confidence: cited
  path: .github/workflows/release.yml
- confidence: cited
  path: AGENTS.md
- confidence: cited
  path: README.md
- confidence: inferred
  path: contracts/substrate-wire/0.23.0
- confidence: inferred
  path: crates/substrate-daemon/src/app/operations.rs
- confidence: cited
  path: crates/substrate-daemon/tests/runtime_vectors.rs
- confidence: inferred
  path: crates/substrate-host/src/probe.rs
- confidence: cited
  path: crates/substrate-wire/src/lib.rs
- confidence: cited
  path: scripts/gate.sh
- confidence: inferred
  path: xtask/bundle-source/0.23.0
- confidence: cited
  path: xtask/src/bundle.rs
- confidence: inferred
  path: xtask/src/bundle/read_only_roots.rs
revision: 8
transitions:
- {from: "draft", to: "proposed", at: "2026-10-23T06:27:15Z", actor: "agent:b10x-chef-orchestrator", revision: 6}
- {from: "proposed", to: "active", at: "2026-10-23T06:27:15Z", actor: "agent:b10x-chef-orchestrator", revision: 7}
- {from: "active", to: "implemented", at: "2026-10-23T14:58:54Z", actor: "agent:b10x-chef-orchestrator", revision: 8, decided_on: {"recorded":{"test_result":1,"review_outcome":3,"verification":2}}}
---
# Story: the published bundle states read-only roots

## Problem

The wire crate accepts `read_only_roots` on exec start (`ExecStartInput::read_only_roots`,
`crates/substrate-wire/src/lib.rs`) and the host binds them (ADR 0010), but no released bundle
through `0.18.0` publishes the field: it is absent from the exec-start input schema, from the
applied-confinement schema, and from the capability snapshot, which states no root bound. A
consumer that reads only the bundle cannot know roots exist. Found while adding `read_only_files`
to `0.18.0`, which does publish its field, fact and observation.

## Contract

A successor bundle adds `read_only_roots` to the exec-start input schema, the applied-confinement
observation and a capability fact carrying `MAX_READ_ONLY_ROOTS`, each bound to the wire crate's
constants and checked by the successor's additions check. Additive; ADR 0010 already decides the
behaviour.

## Acceptance

- The successor bundle's exec-start schema accepts the roots the wire crate accepts and rejects a
  list over `MAX_READ_ONLY_ROOTS`.
- The capability snapshot schema states the served roots bound, and the daemon publishes it.
- The applied-confinement schema lists every bound root, and a runtime vector shows it.

## Scope

Derived 2026-10-23 by `story-scoper`. Each line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** successor bundle `xtask/bundle-source/0.23.0` rendered to `contracts/substrate-wire/0.23.0` — inferred (no bundle `0.18.0`–`0.22.0` mentions `read_only_roots` or `MAX_READ_ONLY_ROOTS`)
- **Files:** `crates/substrate-wire/src/lib.rs` (`MAX_READ_ONLY_ROOTS` :72, `AppliedConfinement.read_only_roots` :1468, `CapabilityFacts` :3188, `ADVERTISED_CONTRACT_BUNDLE` :18) — cited
- **Files:** `xtask/src/bundle.rs` (`check_additions` :894, a `0.23.0` branch beside `check_read_only_file_additions` :1392) — cited
- **Files:** `crates/substrate-daemon/tests/runtime_vectors.rs` (roots vectors :4747, :5589; case counts), `scripts/gate.sh:34-36`, `.github/workflows/release.yml` — cited
- **Files:** `AGENTS.md`, `CHANGELOG.md`, `README.md` — cited (each moved in the `0.22.0` cut `a28b6e78`)
- **Also likely:** `crates/substrate-host/src/probe.rs:157` (the new fact), `xtask/src/bundle/read_only_roots.rs`, `crates/substrate-daemon/src/app/operations.rs:772-787` — inferred
- **Confidence:** medium — the fact name and struct are not decided anywhere; ADR 0010 (`adr/0010-declared-host-roots-are-mounted-read-only.md:79`) says the bundle and capability snapshot grow this field
- **Would collide with:** any unit cutting a bundle or moving `ADVERTISED_CONTRACT_BUNDLE`, adding a `CapabilityFacts` field, or changing runtime-vector case counts
- **Safety fact (unproven):** `CapabilityFacts` is `#[serde(deny_unknown_fields)]` (`lib.rs:3187-3188`); pinned consumers must move with the new fact, as for `exec.read-only-files`.
