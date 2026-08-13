# Design 02: driver and capability model

**Status:** draft for review · **Date:** 2026-08-13

One substrate contract must remain truthful across host, Docker, and later Kubernetes backends.
Drivers are repository-owned adapters behind closed ports, not plugins, caller choices, or separate
products.

## 1. Selection and composition

A deployment configuration selects installed drivers at startup. Each resource records the driver
that owns it, but ordinary client commands target resource identities and requirements rather than a
driver name. Creation may carry capability requirements; deployment policy selects an eligible
configured driver.

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

Every command maps to a required capability predicate. Admission evaluates it before materializing
secrets or changing state. If no configured driver proves the requirement, the answer is `unserved`.
If a driver normally serves it but a request violates a guard or local policy, the answer is
`refused`. Capacity pressure is `exhausted`; machinery failure after acceptance is `failed`.

There is no fallback from a stronger requested sandbox to a weaker driver, no automatic local Flux
path, and no caller-supplied executable used as a driver.

## 5. Compatibility

Driver conformance is proven against shared black-box fixtures for:

- idempotent operation replay;
- observed-state re-read;
- refusal and error classification;
- confinement and environment clearing;
- output truncation without child deadlock;
- lease/cancellation transitions;
- event ordering and resource provenance.

A driver-specific extension first appears as a capability fact. It enters the shared wire only when
at least one consumer journey needs it and unsupported drivers can refuse it coherently.

## Decisions required before implementation

1. Whether v1 supports multiple active drivers in one daemon or exactly one selected driver.
2. The canonical capability document and predicate syntax.
3. Persistence ownership for resource/operation metadata versus driver observation.
4. Startup behavior when an optional driver probe fails.
