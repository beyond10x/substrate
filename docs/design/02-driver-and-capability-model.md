# Design 02: driver and capability model

**Status:** accepted v1 design · **Date:** 2026-08-13

One substrate contract must remain truthful across host, Docker, and later Kubernetes backends.
Drivers are repository-owned adapters behind closed ports, not plugins, caller choices, or separate
products.

## 1. Selection and composition

A v1 daemon selects exactly one active driver at startup. Each resource records that driver and its
capability snapshot, but ordinary client commands target resource identities and requirements rather
than a driver name. Multi-driver placement is a later cloud/composition concern, not a hidden local
scheduler.

The initial daemon contains one host driver. Docker is the second proof of the port boundary.
Kubernetes remains a later target and cannot distort the host contract before a concrete journey
requires it.

## 2. Driver ports

| Port | Required behavior |
|---|---|
| machine probe | verify enforcement mechanisms, versions, limits, and availability |
| workspace | materialize, inspect, mutate, snapshot, and destroy a confined tree |
| exec | start argv-only work, observe, signal, capture bounded output, and open a PTY |
| workload | deploy and reconcile long-lived image-backed work where served |
| image | build, inspect, pull, and push where served |
| volume | create, inspect, attach, detach, and destroy storage where served |
| endpoint | expose bounded addresses and open direct tunnels where served |

Ports return typed observations and driver errors. They never return preformatted HTTP errors or
consumer-specific vocabulary.

## 3. Capability facts

A capability is published only after its probe succeeds. Facts are closed, typed, and versioned.
Each probe result has a snapshot id bound to driver identity, backend/configuration generation, and
probe time. A backend replacement or relevant configuration change invalidates the snapshot.
Working groups include:

- `workspace`: confinement mode, snapshots, maximum file/range sizes;
- `exec`: argv spawn, applied sandbox backends, network isolation, PTY, output limits;
- `workload`: availability, replace mode, restart support, resource limits;
- `image`: pull/push/build and whether builds are confined;
- `volume`: availability, attachment rules, quota support;
- `endpoint`: loopback, LAN, tunnel, and exposure restrictions;
- `observe`: ledger retention, event retention, and metrics support.

Configured intent is not a fact. A present-but-unusable Docker socket, broken sandbox binary, or
unreachable cluster produces an absent capability plus a diagnostic; it cannot produce an optimistic
capability.

## 4. Admission

Every command maps to a required capability predicate. Admission evaluates it against one selected
driver snapshot before materializing secrets or changing state, binds that snapshot to the
operation, and rechecks security-critical predicates immediately before dispatch. If no configured
driver proves the requirement, the answer is `unserved`.
If a driver normally serves it but a request violates a guard or local policy, the answer is
`refused`. Capacity pressure is `exhausted`; machinery failure after acceptance is `failed`.

There is no fallback from a stronger requested sandbox to a weaker driver, no automatic local Flux
path, and no caller-supplied executable used as a driver.

## 5. Compatibility

Before a driver may be declared conformant, its implementing phase must prove these properties
against shared black-box fixtures:

- idempotent operation replay;
- observed-state re-read;
- refusal and error classification;
- confinement and environment clearing;
- output truncation without child deadlock;
- lease/cancellation transitions;
- event ordering and resource provenance.

A driver-specific extension first appears as a capability fact. It enters the shared wire only when
at least one consumer journey needs it and unsupported drivers can refuse it coherently.

## V1 decisions

1. **Selection:** exactly one active driver per daemon. The minimum slice selects `host`.
2. **Capability wire:** `/v1/machine` publishes `{snapshot, driver, driver_version,
   config_generation, probed_at, valid_until?, facts}`. Facts are a closed map from namespaced fact
   name to a JSON scalar or closed object. A request supplies a conjunction of typed predicates
   `{fact, op: eq|one_of|lte|gte, value}`; unknown facts/operators are refused, never ignored.
3. **Persistence:** the substrate domain store owns resource identity, desired command, operation
   ledger, applied-policy record, and event journal. Drivers own external execution state and return
   observations; the service reconciles rather than treating its metadata as driver truth.
4. **Probe failure:** failure of the selected driver's mandatory probe leaves the daemon unready and
   its operations unserved. A daemon may expose authenticated health diagnostics but cannot publish
   optimistic capabilities. Optional facts are simply absent with a diagnostic.
5. **Invalidation:** every driver/config/backend change changes `config_generation` and invalidates
   snapshots. The snapshot digest binds driver kind/version, configuration generation, probe time,
   canonical backend paths, backend file identity and SHA-256, cgroup-root identity/controllers,
   and capability facts. Operations bind the admitted snapshot. Immediately before dispatch the
   daemon recomputes the security-critical backend binding; mismatch is `refused` before secret
   acquisition or dispatch.
