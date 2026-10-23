---
format: aep.planning-md/3
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
scope:
- confidence: inferred
  path: Cargo.toml
- confidence: inferred
  path: adr/README.md
- confidence: inferred
  path: crates/substrate-daemon/src/app/health.rs
- confidence: inferred
  path: crates/substrate-daemon/src/app/routes.rs
- confidence: inferred
  path: crates/substrate-daemon/src/main.rs
- confidence: inferred
  path: crates/substrate-daemon/src/runtime.rs
- confidence: inferred
  path: crates/substrate-host/src/probe.rs
- confidence: inferred
  path: crates/substrate-store/src/schema.rs
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-08-31T22:54:32Z", actor: "human:timo", revision: 2, imported: true}
---
# Story: Operators get safe health and service telemetry

## Outcome

Kubernetes and service operators can tell whether a daemon process is alive, ready to admit durable work and healthy under load without scraping product execution records.

## Acceptance

A liveness endpoint reports only process-loop health. Readiness checks durable store access, schema compatibility, required driver probes and maintenance-loop viability without mutating caller resources. A separate OpenMetrics endpoint exposes bounded service counters, histograms and gauges for request outcomes, refusal classes, operation latency, queue saturation, ledger maintenance, driver dispatch and process resource use. Labels exclude subject, tenant, workspace, exec, operation, argv, path, destination, token and secret data. Tests assert metric-name stability, bounded label sets, no sensitive values, readiness transitions during store/driver failure, and correct Kubernetes probe behavior.

## Contract boundary

The operational endpoint is distinct from the authenticated per-execution metrics routes in the development wire contract. It does not add a second product observation ledger or promise mean-memory estimates the kernel does not supply exactly.

## Scope

Derived 2026-10-23 by `story-scoper`. Each line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** `crates/substrate-daemon` (listener wiring and new endpoint handlers) — inferred; the story names no file
- **Files:** `crates/substrate-daemon/src/runtime.rs` (`serve` :550, `MaintenanceTasks` :1697), `crates/substrate-daemon/src/main.rs` (beside `tls_listen` :395), `crates/substrate-daemon/src/app/routes.rs`, a new `app/health.rs` — inferred
- **Also likely:** `crates/substrate-store/src/schema.rs`, `crates/substrate-host/src/probe.rs`, `Cargo.toml` (an OpenMetrics encoder), `adr/README.md` — inferred
- **Confidence:** low — no health, readiness or OpenMetrics code exists (`git grep -i 'healthz|readyz|openmetrics|prometheus'`)
- **Blocker:** no ADR or design document covers operational endpoints (invariant 8). ADR 0021 covers execution facts only. The document must fix the listener bind and authentication, endpoint paths, metric names, label bounds and readiness semantics.
