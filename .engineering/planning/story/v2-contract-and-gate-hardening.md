---
format: aep.planning-md/3
id: story:v2-contract-and-gate-hardening
kind: story
status: implemented
title: The successor bundle declares every served API major and closes compatibility gaps
summary: Contract v2 workspace routes, wildcard paths, predecessor order and transitive schemas.
relations:
- decomposes: epic:release-hardening
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-08-31T08:37:44Z", actor: "human:timo", revision: 2, imported: true}
- {from: "proposed", to: "active", at: "2026-08-31T08:37:44Z", actor: "human:timo", revision: 3, imported: true}
- {from: "active", to: "implemented", at: "2026-08-31T22:50:00Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}, imported: true}
---
# Story: V2 contracts and compatibility hardening

## Outcome

Every served route is declared in a multi-major successor bundle and compatibility checks see paths, adjacency and transitive schema changes.

## Acceptance

- Five v2 routes are registered with api major 2 and v2 envelopes.
- V1 bytes and all frozen bundles remain unchanged.
- A versioned renderer supports catch-all paths without changing render.rs.
- Adjacent predecessor, served paths and recursive ref closures are enforced.
