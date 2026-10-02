---
format: aep.planning-md/3
id: epic:resource-bounded-execution
kind: epic
status: implemented
title: Resource-bounded general execution
summary: Hard persistent and ephemeral storage ceilings, exact execution accounting, and public runnable examples for arbitrary confined commands.
owner: substrate
tags:
- confinement
- o1
- observability
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-08-31T19:52:46Z", actor: "human:timo", revision: 2, imported: true}
- {from: "proposed", to: "active", at: "2026-08-31T19:52:46Z", actor: "human:timo", revision: 3, imported: true}
- {from: "active", to: "implemented", at: "2026-08-31T22:50:10Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}, imported: true}
---
## Outcome

Substrate runs ordinary argv-based workloads with hard byte and inode ceilings on every persistent or disk-backed writable surface, reports exact kernel observations during and after the run, and teaches a public reader how to use those guarantees without implying a scheduler or policy engine.

## Boundaries

This epic adds host-driver capability and development-contract behavior. It does not add product policy, fleet scheduling, mean-memory estimates, Prometheus exposition, a stable bundle release, or a contract-header migration.
