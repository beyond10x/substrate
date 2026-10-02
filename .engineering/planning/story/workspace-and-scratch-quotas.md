---
format: aep.planning-md/3
id: story:workspace-and-scratch-quotas
kind: story
status: implemented
title: Workspaces and exec scratch carry hard storage quotas
summary: Quota-enabled hosts enforce declared byte and inode ceilings on persistent workspaces and per-exec /scratch without scan-based approximation.
owner: substrate
tags:
- host
- o1
- wire
relations:
- decomposes: epic:resource-bounded-execution
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-08-31T19:52:46Z", actor: "human:timo", revision: 2, imported: true}
- {from: "proposed", to: "active", at: "2026-08-31T19:52:46Z", actor: "human:timo", revision: 3, imported: true}
- {from: "active", to: "implemented", at: "2026-08-31T22:50:00Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}, imported: true}
---
## Intent

Add optional persistent workspace and ephemeral exec-scratch limits. The host advertises the capability only after proving project-quota byte, inode and inheritance enforcement over an operator-reserved project-id range.

## Design prerequisite

Implementation follows `docs/design/17-resource-accounting-and-storage-quotas.md` and ADR 0020. Frozen bundles remain untouched; bundle 0.11.0 is the successor.

## Acceptance

Quota identity is durable before dispatch, unsupported hosts refuse by name, concurrent writers share the same hard workspace ceiling, scratch is mounted only at `/scratch`, and allocation identities are reused only after bounded cleanup proves absence and zero usage.
