---
format: aep.planning-md/1
id: story:operational-health-and-telemetry
kind: story
status: proposed
title: Operators get safe health and service telemetry
summary: Dedicated liveness, readiness and OpenMetrics endpoints report service health without leaking subjects, credentials or high-cardinality resource identifiers.
owner: substrate
tags:
- observability
- operations
- remote
relations:
- decomposes: epic:remote-serving
- depends_on: story:production-tls-control-listener
revision: 2
---
# Story: Operators get safe health and service telemetry

## Outcome

Kubernetes and service operators can tell whether a daemon process is alive, ready to admit durable work and healthy under load without scraping product execution records.

## Acceptance

A liveness endpoint reports only process-loop health. Readiness checks durable store access, schema compatibility, required driver probes and maintenance-loop viability without mutating caller resources. A separate OpenMetrics endpoint exposes bounded service counters, histograms and gauges for request outcomes, refusal classes, operation latency, queue saturation, ledger maintenance, driver dispatch and process resource use. Labels exclude subject, tenant, workspace, exec, operation, argv, path, destination, token and secret data. Tests assert metric-name stability, bounded label sets, no sensitive values, readiness transitions during store/driver failure, and correct Kubernetes probe behavior.

## Contract boundary

The operational endpoint is distinct from the authenticated per-execution metrics routes in the development wire contract. It does not add a second product observation ledger or promise mean-memory estimates the kernel does not supply exactly.
