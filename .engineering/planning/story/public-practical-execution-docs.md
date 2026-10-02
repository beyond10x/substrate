---
format: aep.planning-md/3
id: story:public-practical-execution-docs
kind: story
status: implemented
title: The public site teaches practical confined execution
summary: Runnable terminal guides show arbitrary commands, storage and process bounds, output recovery and exact resource observations for agent and non-agent workloads.
owner: substrate
tags:
- docs
- o1
- website
relations:
- decomposes: epic:resource-bounded-execution
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-08-31T19:52:46Z", actor: "human:timo", revision: 2, imported: true}
- {from: "proposed", to: "active", at: "2026-08-31T19:52:46Z", actor: "human:timo", revision: 3, imported: true}
- {from: "active", to: "implemented", at: "2026-08-31T22:49:58Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}, imported: true}
---
## Intent

Turn the public site from a primarily conceptual overview into a hands-on guide for platform engineers while retaining the public projection boundary.

## Acceptance

The site covers agent execution, build and test workers, converters, plugin runners, restricted automation and interactive terminals; every command matches the development contract; shipped, development and absent behavior remain visually distinct; and no internal design, ADR, planning or review material is published or linked.
