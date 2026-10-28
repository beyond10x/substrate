---
status: accepted
date: 2026-10-28
---

# ADR 0045: operational health and metrics use a separate listener outside the wire contract

Decides `story:operational-health-and-telemetry`. **Accepted** by the operator on 2026-10-28
with the recommended option of each question in § *Decisions for the operator*: D1 to D6 are all
A, so § *Decision* stands as written.

## Context

- **Nothing exists yet.** No liveness, readiness or OpenMetrics code is in the tree. Outside the
  planning store, `git grep -il 'healthz\|readyz\|openmetrics\|prometheus'` matches only
  `docs/design/01-contract.md` and `docs/design/17-resource-accounting-and-storage-quotas.md`.
- **Three control postures, one per daemon.** `serve` (`crates/substrate-daemon/src/runtime.rs:567`)
  selects the Unix socket with kernel peer credentials, the development static-bearer TCP listener,
  or the production TLS listener (`serve_tls`, `runtime.rs:1373`). TLS and hosted Identity require
  each other (`runtime.rs:573-577`).
- **Probes cannot pass hosted admission.** On the TLS listener every request goes through
  `require_hosted_authority` (`crates/substrate-daemon/src/hosted.rs:518`), which resolves the
  request's bearer credential online at Identity (ADR 0026,
  [ADR 0040](0040-hosted-admission-resolves-every-request-online-over-a-bounded-connection-pool.md)
  decision 1: nothing is cached). A kubelet `httpGet` probe cannot present such a credential. A
  scraper could, but each scrape would then cost one Identity resolution.
- **No production plaintext.** [ADR 0024](0024-production-network-control-uses-server-authenticated-tls.md)
  :26-27: "There is no production plaintext fallback". The ADR is about network *control*: its
  context (:10-19) is the static-bearer TCP listener and hosted callers sending control-plane
  bytes.
- **A route on the control router is a wire change.**
  [ADR 0018](0018-one-registry-declares-every-served-api-major.md):30: "One bundle describes every
  route the daemon answers". `router_for` (`crates/substrate-daemon/src/app/routes.rs:55`) mounts
  exactly the registry's routes, so a health route there forces a successor bundle
  (invariant 6).
- **The design text defers this.** `docs/design/01-contract.md:212-213`: "Prometheus exposition is
  a later telemetry adapter and does not add a second v1 control listener". `:364`: "Metrics: JSON
  on the authenticated control API. Prometheus is a later adapter/listener."
- **Per-execution metrics are a different surface.**
  [ADR 0021](0021-execution-metrics-are-explicit-exact-observations.md) serves exact per-exec
  observations on `/v1/metrics` and `/v1/metrics/stream` (`routes.rs:58-59`) under the `observe`
  scope (`hosted.rs:479-480`). Design 17 (`docs/design/17-resource-accounting-and-storage-quotas.md:99-100`)
  defers retained telemetry.
- **What readiness would read.** The two maintenance loops, the lease sweeper and the containment
  step, tick every 250 ms (`MaintenanceTasks`, `runtime.rs:1722-1758`). Nothing records when a round
  last completed, so a stalled loop is invisible today. The driver's facts are the snapshot
  `Driver::machine` returns (`crates/substrate-host/src/lib.rs:572`), probed once at startup. Store
  schema compatibility is checked only when the store opens (`Store::open_with_config`,
  `crates/substrate-store/src/schema.rs:32`).
- **Where saturation is.** The transport bounds are `UnixTransportPolicy::production`
  (`runtime.rs:52-55`: 128 connections, 32 per uid) and `TcpConnectionLimits::production`
  (`runtime.rs:323-330`: 128 connections, 16 per source address).

## Decision

1. **A separate operational listener, off by default.** `--ops-listen <address:port>` (environment
   `SUBSTRATE_OPS_LISTEN`) binds it. It has no default address, may be combined with any control
   posture, and must differ from every control listener address. It serves HTTP/1.1 `GET` on
   exactly three paths. Every other method or path answers `404` with an empty body. It reads no
   `Authorization` header, accepts no request body and no upgrade, and has its own connection bound
   (8 concurrent connections, a 2 s header deadline), so probes and scrapes never take a control
   listener's permit.
2. **It is not a control listener and not part of the wire contract.** It serves no route of the
   registry, so `docs/design/01-contract.md:212-213` ("does not add a second v1 control listener")
   still holds. ADR 0018's registry describes the control listeners' routes, and this listener
   serves none of them. No bundle changes. The paths and the metric catalogue below are documented
   here and in the deployment guide, not in a bundle.
3. **Transport and authentication: plaintext, unauthenticated** (D1 A, D2 A). This ADR reads
   ADR 0024's "no production plaintext fallback" as applying to control traffic. The operational
   listener is not a fallback for it: it carries no control route, reads no credential and answers
   with no identifier. Exposure is bounded by the bind address and, in Kubernetes, by NetworkPolicy
   ([ADR 0046](0046-kubernetes-serves-one-node-bound-daemon-per-stable-pod-address.md)).
4. **Paths** (D3 A): `/livez`, `/readyz`, `/metrics`.
5. **Liveness reports the process loop only.** `/livez` answers `200 ok` when its handler runs and
   neither maintenance task has *finished*. Both are infinite loops, so a finished task has panicked
   or been aborted. It checks no store, driver or Identity, because a dependency outage must not
   restart the daemon: a restart drops live sessions and sends every live exec through
   `reconcile_after_restart`.
6. **Readiness checks four things and mutates nothing.** `/readyz` answers `200` when every check
   passes and `503` otherwise. The body is text: one `<check> ok|failed` line per check and nothing
   else. It carries no error text, path or SQL message. The checks:

   | check | passes when |
   |---|---|
   | `store` | one read-only statement on the ledger connection returns within 1 s |
   | `schema` | the schema version that statement reads equals the version this binary opens |
   | `maintenance` | each maintenance task completed a round within the last 5 s (20 ticks) |
   | `driver` | every fact named by `--ops-require-fact` is present in the startup snapshot (D5 A) |

   `--ops-require-fact` is repeatable and empty by default. Readiness does not resolve anything at
   Identity (D6 A). Once the daemon begins shutting down, readiness answers `503` before any
   listener closes.
7. **The metric catalogue is closed** (D4 A for the implementation). Every label takes values from a
   closed set fixed in code. No label may carry a subject, tenant, uid, workspace, exec, session,
   process or operation id, an argv element, a path, a destination, an aperture or Git source name,
   a peer address, a token, a secret or free error text.

   | metric | type | labels and their closed values |
   |---|---|---|
   | `substrate_build_info` | gauge, always 1 | `version`, `contract_bundle`: one series per binary |
   | `substrate_ready` | gauge 0/1 | `check`: `store`, `schema`, `maintenance`, `driver` |
   | `substrate_http_requests_total` | counter | `listener`: `unix`, `tcp-development`, `tls`; `family`: `observe`, `workspaces`, `exec`, `process`, `none` (the scopes of `hosted.rs:478-495`); `outcome`: `ok` or an `ErrorClass` (`crates/substrate-wire/src/lib.rs:888`: `refused`, `conflict`, `unserved`, `exhausted`, `failed`) |
   | `substrate_http_request_duration_seconds` | histogram | `listener`, `family` |
   | `substrate_refusals_total` | counter | `code`: a refusal code the wire crate names as a constant, otherwise `other` |
   | `substrate_dispatch_total` | counter | `family`, `outcome`: driver dispatch after durable admission |
   | `substrate_dispatch_duration_seconds` | histogram | `family` |
   | `substrate_connections_open` | gauge | `listener` |
   | `substrate_connections_limit` | gauge | `listener` |
   | `substrate_connections_rejected_total` | counter | `listener` |
   | `substrate_output_stalls_total` | counter | none: live-output stall terminations ([ADR 0031](0031-live-output-backpressure-has-a-stall-deadline.md)) |
   | `substrate_maintenance_rounds_total` | counter | `task`: `lease-sweep`, `containment` |
   | `substrate_maintenance_round_duration_seconds` | histogram | `task` |
   | `substrate_maintenance_last_round_timestamp_seconds` | gauge | `task` |
   | `substrate_hosted_admissions_total` | counter | `result`: `admitted`, `credential-absent`, `authority-invalid`, `scope-denied`, `authority-unavailable` |
   | `substrate_hosted_resolution_duration_seconds` | histogram | none |
   | `process_cpu_seconds_total`, `process_resident_memory_bytes`, `process_open_fds`, `process_max_fds`, `process_start_time_seconds` | the conventional process metrics, read from `/proc/self` | none |

   Every histogram has the same eleven buckets, from 5 ms to 10 s (0.005, 0.01, 0.025, 0.05, 0.1,
   0.25, 0.5, 1, 2.5, 5, 10). Without `substrate_refusals_total` the catalogue has under 500 series.
   That counter adds at most one series per named refusal code plus `other`. A test pins the
   exact maximum. Queue saturation is measured at the transport bounds and the output stall, never
   per session.
8. **Superseded design text.** On acceptance, the second sentence of
   `docs/design/01-contract.md:212-213` and the second sentence of `:364` are replaced by a pointer
   to this ADR. The first sentences stay: per-resource metrics remain JSON on the authenticated
   control API (ADR 0021). ADR 0024 and ADR 0018 stay accepted. This ADR states the reading of each
   that it relies on (decisions 2 and 3) and supersedes neither.

## Decisions for the operator

### D1 — transport of the operational listener

| option | what it does | what it costs |
|---|---|---|
| **A** plaintext HTTP on an explicit address | kubelet `httpGet` and any scraper reach it directly. ADR 0024's rule is read as covering control traffic (decision 3) | anyone who reaches the port can read the counters. Scraped values have no integrity protection. Exposure rests on the bind address and NetworkPolicy |
| **B** TLS with the control listener's certificate snapshot, no client authentication | kubelet probes with `scheme: HTTPS`, which skips verification. ADR 0024 holds literally | exists only beside `--tls-listen`, so a Unix-socket daemon gets no endpoints. A second reader of the key snapshot. A scraper that verifies needs each pod's own server name (per-pod SANs, ADR 0046), so probes, and often scrapes, still authenticate nothing |
| **C** loopback or a Unix socket only | the endpoints never leave the pod. Probes become `exec` probes, scraping goes through a sidecar | an exec probe starts a process every period. The image has no HTTP client beyond `socat` and `bash`. A sidecar adds a container with its own exposure |

**Recommendation: A.** The listener carries no control route and nothing that names a caller. TLS
would add a second key reader and still leave the probes unauthenticated.

### D2 — authentication of the operational listener

| option | what it does | what it costs |
|---|---|---|
| **A** none | all three paths are open. Exposure is bounded by the bind address and NetworkPolicy, content by the closed catalogue | the counts (request rates, refusal codes) are readable by any peer that reaches the port |
| **B** a static bearer file for `/metrics` only | scrapers present a bearer; the probes stay open | a second credential to provision and rotate, and two admission regimes on one listener, to protect counts |
| **C** hosted Identity authority, scope `observe`, for `/metrics` | the scraper is an Identity principal | one Identity resolution per scrape (ADR 0040, no cache). Metrics fail exactly when Identity is down, when they are most needed |

**Recommendation: A.** No authentication kubelet can present exists, and B or C would protect only
counts while splitting the listener into two regimes.

### D3 — endpoint paths

| option | what it does | what it costs |
|---|---|---|
| **A** `/livez`, `/readyz`, `/metrics` | the paths Kubernetes components and Prometheus scrapers use by default | none beyond fixing three names |
| **B** `/healthz`, `/readyz`, `/metrics` | the older single health path | Kubernetes components deprecated `/healthz` in favour of `/livez` and `/readyz` |
| **C** a versioned prefix, `/ops/v1/…` | room for an incompatible successor | every probe and scrape configuration carries the prefix. The contract that can change is the metric names, not the paths |

**Recommendation: A.** These are the defaults every consumer already expects.

### D4 — how the exposition is produced

| option | what it does | what it costs |
|---|---|---|
| **A** a hand-written OpenMetrics text encoder over a static catalogue | label values are Rust enums, so the bound is a type. No new dependency | the encoder and fixed-bucket histograms are maintained here (on the order of a few hundred lines) |
| **B** the `prometheus-client` crate | a maintained OpenMetrics encoder with typed label sets | new dependencies through `check-licenses`, `check-advisories` and `THIRD_PARTY_LICENSES.html` |
| **C** the `metrics` facade with `metrics-exporter-prometheus` | a global recorder, called from anywhere | labels are strings at each call site, so the bound is not a type. The exporter brings its own HTTP stack, and whether that stack pulls in HTTP/2, which `check-advisories` refuses, is unverified |

**Recommendation: A.** The catalogue is small and closed, and a type-level label bound is the safety
property the story asks for.

### D5 — what readiness reads from the driver

| option | what it does | what it costs |
|---|---|---|
| **A** the startup snapshot, against facts the operator names with `--ops-require-fact` | a daemon started without a required fact (for example, no confinement) is never ready, so a misconfigured node is visible. No `Driver` trait change | a fact lost after startup (say, cgroup delegation removed) goes unseen until restart |
| **B** a background re-probe every 60 s, with readiness reading its last result | detects facts lost at runtime | changes the `Driver` trait, which 14 files implement, and `crates/substrate-host/src/probe.rs`. A re-probe spawns sandbox processes on a timer |
| **C** a re-probe on every readiness request | the freshest answer | spawns sandboxes at the kubelet's probe rate. A slow probe makes readiness flap |

**Recommendation: A**, with B as a labelled later milestone. A covers the misconfiguration case
without touching the driver contract.

### D6 — whether readiness depends on Identity

| option | what it does | what it costs |
|---|---|---|
| **A** no: Identity reachability is a metric (`substrate_hosted_admissions_total{result="authority-unavailable"}`), not a readiness check | during an Identity outage each pod stays addressable and answers the named refusal `auth.authority-unavailable` | a load balancer keeps sending requests that will be refused |
| **B** yes: readiness resolves a probe credential at Identity | an Identity outage takes every pod out of service | every pod turns unready at once. ADR 0046 publishes a pod's DNS record only while it is ready, so callers get a name-resolution failure instead of a named refusal. Readiness also needs a credential of its own |

**Recommendation: A.** A named refusal says more than a vanished address, and one dependency's
outage must not empty every pod at once.

## Consequences

- The daemon gains one flag, one listener task and a static metric registry held by `App`. The
  instrumentation points are the request layer, the refusal and store-failure responses, dispatch
  completion, the maintenance loops and the transport permits. The listener serves no wire route,
  so no contract bundle, SDK or vector changes.
- Two tests decide the safety claims, both in an existing shipped-daemon test binary. The first
  sends requests whose subject, path, workspace name and bearer are recognisable strings, then
  asserts that none of those strings appears in the `/metrics` bytes, and that the series count
  stays within the pinned maximum. The second asserts that every registry route answers `404` on
  the operational listener and that no response there depends on an `Authorization` header.
- Readiness transitions are tested against an unreadable store and a stopped maintenance task, each
  scenario run once. The Kubernetes probe behaviour is exercised by ADR 0046's chart lane.
- The metric names and label sets are pinned by a test against this catalogue. Renaming a metric,
  or changing a label set, changes that test and the deployment guide together and is recorded in
  `CHANGELOG.md`.
- The schema check needs a read-only version query that `substrate-store` does not expose today.

## What this does not decide

- Traces, OTLP export, log shipping, alert rules and dashboards.
- Retained time series or means. ADR 0021 still holds: Substrate supplies exact counters, and an
  external system computes rates, percentiles and history.
- Per-exec, per-workspace or per-subject telemetry on this listener. That stays on the
  authenticated control API.
- Re-probing driver facts at runtime (D5 B), and authentication on this listener at a later date.
- Any wire change. None of the options above needs one. Mounting the endpoints on `router_for`
  would need a successor bundle and is not offered.

## Addendum, 2026-10-28: review pass 1

Two bounds tightened at implementation, after the security and adversary reviews
(`review-result:ops-listener-security-pass-1`, `review-result:ops-listener-adversary-pass-1`):

- Of decision 1's 8 concurrent connections, at most 4 come from one source address
  (`TcpConnectionLimits::operational()`), so one peer cannot hold every slot against kubelet.
- The schema check reads a stamp; it never writes one. A ledger stamped by a newer binary is refused
  at open (`StoreError::NewerSchema`) instead of being re-stamped and reported current.
