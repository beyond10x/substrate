---
format: aep.planning-md/1
id: story:v2-contract-and-gate-hardening
kind: story
status: active
title: The successor bundle declares every served API major and closes compatibility gaps
summary: Contract v2 workspace routes, wildcard paths, predecessor order and transitive schemas.
relations:
- decomposes: epic:release-hardening
revision: 3
---
# Story: V2 contracts and compatibility hardening

## Outcome

Every served route is declared in a multi-major successor bundle and compatibility checks see paths, adjacency and transitive schema changes.

## Acceptance

- Five v2 routes are registered with api major 2 and v2 envelopes.
- V1 bytes and all frozen bundles remain unchanged.
- A versioned renderer supports catch-all paths without changing render.rs.
- Adjacent predecessor, served paths and recursive ref closures are enforced.
