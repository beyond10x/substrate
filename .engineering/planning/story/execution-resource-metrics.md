---
format: aep.planning-md/3
id: story:execution-resource-metrics
kind: story
status: implemented
title: Execs expose exact live and terminal resource usage
summary: Opted-in runs expose monotonic wall time and kernel CPU, memory, process and I/O observations through GET and latest-wins WebSocket delivery.
owner: substrate
tags:
- daemon
- o1
- wire
relations:
- decomposes: epic:resource-bounded-execution
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-08-31T19:52:46Z", actor: "human:timo", revision: 2, imported: true}
- {from: "proposed", to: "active", at: "2026-08-31T19:52:46Z", actor: "human:timo", revision: 3, imported: true}
- {from: "active", to: "implemented", at: "2026-08-31T22:49:56Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}, imported: true}
---
## Intent

Add explicit per-exec resource measurement, a read route and a latest-wins live stream. The terminal observation is durable; mean memory and retained time series stay absent because neither is an exact kernel terminal fact.

## Design prerequisite

Implementation follows `docs/design/17-resource-accounting-and-storage-quotas.md` and ADR 0021. Frozen bundles remain untouched; bundle 0.11.0 is the successor.

## Acceptance

Legacy request bytes remain unchanged, live observation cannot backpressure the run, terminal counters are captured before cgroup cleanup, restart gaps are named rather than filled, and timeout, CPU and OOM termination have distinct recorded refusals.
