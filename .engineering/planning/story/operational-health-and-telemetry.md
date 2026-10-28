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
revision: 5
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

## Scope

Derived 2026-10-27 by `story-scoper` at c2b95f28 and updated at 4decf8d1 for the proposed ADR. Every
line is **cited** (read from the story or the tree) or **inferred** (a reading that could be wrong).

- **Primary surface:** `crates/substrate-daemon` listener wiring, a new operational listener beside `serve_tls`, as proposed by ADR 0045 (`adr/0045-operational-health-and-metrics-use-a-separate-listener.md`, status `proposed`) — inferred; no health, readiness or OpenMetrics code exists (`git grep -il 'healthz\|readyz\|openmetrics\|prometheus'` matches only plans and design text)
- **Files:** `crates/substrate-daemon/src/runtime.rs` — `serve` :567 (listener selection), `serve_tls` :1373, `MaintenanceTasks` :1722 (the maintenance-loop viability readiness must report; ADR 0045 adds a last-round timestamp per task) — cited, read in the tree
- **Files:** `crates/substrate-daemon/src/main.rs` — `Arguments` :228, beside `tls_listen` :407, for `--ops-listen` and `--ops-require-fact` (ADR 0045 decisions 1 and 6); `DaemonConfig` in `runtime.rs` :393 — inferred
- **Files:** a new `crates/substrate-daemon/src/app/health.rs` for the liveness, readiness and OpenMetrics handlers and the closed metric catalogue (ADR 0045 decision 7, D4 A: hand-written encoder, no new dependency); `crates/substrate-daemon/src/app.rs` module list — inferred
- **Files:** `crates/substrate-daemon/src/app/routes.rs` (`router_for` :55, a request-outcome layer; the operational routes are **not** mounted here), `app/responses.rs` (`failure` :218, `store_failure` :199, refusal classes), `app/operations.rs` (`refuse_before_dispatch` :1088, `finish_dispatch_*` :1489-1611, dispatch outcome and latency), `app/service.rs` (`App` :166 holds the registry, `sweep_expired` :394 for ledger maintenance), `crates/substrate-daemon/src/hosted.rs` (`require_hosted_authority` :518, admission results for `substrate_hosted_admissions_total`) — inferred, the instrumentation points
- **Symbols:** `serve`, `serve_tls`, `MaintenanceTasks`, `router_for`, `App`, `require_hosted_authority`, `Driver::machine` (`crates/substrate-host/src/lib.rs:572`, read from the startup snapshot under D5 A; no `Driver` trait change) — cited
- **Tests:** an existing shipped-daemon binary, `crates/substrate-daemon/tests/tls_listener.rs`, rather than a new one (merge 11d12ee3 folded test binaries to shorten the gate) — inferred
- **Also likely:** `crates/substrate-store/src/schema.rs` (`Store::open_with_config` :32; a read-only schema-version query for the `schema` readiness check) — inferred. `Cargo.toml`, `crates/substrate-daemon/Cargo.toml`, `Cargo.lock` and `THIRD_PARTY_LICENSES.html` only if the operator picks D4 B or C over the recommended hand-written encoder — inferred
- **Documents:** `adr/0045-operational-health-and-metrics-use-a-separate-listener.md` and its `adr/README.md` row (drafted, `proposed`; `cargo xtask check-adrs` refuses `proposed` until acceptance); `docs/design/01-contract.md` :212-213 and :364, whose second sentences ADR 0045 decision 8 replaces with a pointer on acceptance; `STATUS.md`, `CHANGELOG.md`, `README.md`, `website/docs/guides/deployment.md` (the listen flags, paths and metric catalogue) — inferred
- **Confidence:** medium — the listener and maintenance sites are read in the tree; the instrumentation points wait on the operator's answers to ADR 0045 D1-D6
- **Would collide with:** any unit touching `runtime.rs` `serve`/`serve_tls`, `main.rs` `Arguments`, the `App` struct in `app/service.rs`, or the shared response helpers in `app/responses.rs`; `story:node-bound-kubernetes-serving-profile` (ADR 0046) consumes this story's paths and port for its probes (ADR 0046 K8 A) and shares `adr/README.md`, `STATUS.md`, `CHANGELOG.md` and `website/docs/guides/deployment.md`
- **Safety fact:** the operational endpoints stay out of the wire contract only if they are not mounted on `router_for`. ADR 0018 :30 says "One bundle describes every route the daemon answers", so a route there forces a successor bundle (`adr/0018-one-registry-declares-every-served-api-major.md:30`, `crates/substrate-daemon/src/app/routes.rs:55`). ADR 0045 decision 2 puts them on a separate listener — unproven until a test asserts every registry route answers `404` there
- **Blocker:** ADR 0045 is `proposed`; invariant 8 holds code until the operator decides D1 (transport; recommended plaintext, reading ADR 0024 :26-27 as covering control traffic), D2 (authentication; recommended none), D3 (paths; recommended `/livez`, `/readyz`, `/metrics`), D4 (encoder), D5 (driver readiness from the startup snapshot) and D6 (readiness excludes Identity) and the ADR is accepted
