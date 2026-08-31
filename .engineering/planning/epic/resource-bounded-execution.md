---
format: aep.planning-md/1
id: epic:resource-bounded-execution
kind: epic
status: active
title: Resource-bounded general execution
summary: Hard persistent and ephemeral storage ceilings, exact execution accounting, and public runnable examples for arbitrary confined commands.
owner: substrate
tags:
- confinement
- o1
- observability
revision: 3
---
## Outcome

Substrate runs ordinary argv-based workloads with hard byte and inode ceilings on every persistent or disk-backed writable surface, reports exact kernel observations during and after the run, and teaches a public reader how to use those guarantees without implying a scheduler or policy engine.

## Boundaries

This epic adds host-driver capability and development-contract behavior. It does not add product policy, fleet scheduling, mean-memory estimates, Prometheus exposition, a stable bundle release, or a contract-header migration.
