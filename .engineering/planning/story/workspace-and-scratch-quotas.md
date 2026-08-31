---
format: aep.planning-md/1
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
---
## Intent

Add optional persistent workspace and ephemeral exec-scratch limits. The host advertises the capability only after proving project-quota byte, inode and inheritance enforcement over an operator-reserved project-id range.

## Design prerequisite

Implementation follows `docs/design/17-resource-accounting-and-storage-quotas.md` and ADR 0020. Frozen bundles remain untouched; bundle 0.11.0 is the successor.

## Acceptance

Quota identity is durable before dispatch, unsupported hosts refuse by name, concurrent writers share the same hard workspace ceiling, scratch is mounted only at `/scratch`, and allocation identities are reused only after bounded cleanup proves absence and zero usage.
