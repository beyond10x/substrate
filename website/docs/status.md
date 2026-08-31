---
title: Status and limitations
description: What the current Substrate development release serves, and what remains absent.
---

# Development release, not a stable published contract

Substrate has a tagged, keyless-signed daemon image and reproducible development contract bundles.
The daemon distribution is published; stable contract publication still requires separate bundle
signing and digest pinning.

The current implementation is a Linux host slice. Treat every capability as deployment-specific and
verify it through `GET /v1/machine`.

## Served now

| Area | Current state |
|---|---|
| personal transport | owner-permissioned Unix socket with kernel-derived local identity |
| workspaces | empty source, guarded file access, atomic replacement, destruction, leases |
| exec | available only with the complete probed Linux confinement floor |
| durability | operation reservation before dispatch, persisted terminal observations and output |
| recovery | operation lookup, bounded events, reconciliation snapshots, restart-to-unknown |
| limits | body, path, output, retention, concurrency, process, memory, CPU, duration, and probe-gated writable-storage bounds |
| observations | explicit live and terminal wall, CPU, memory peak/current, process, OOM, block-I/O, and scratch usage facts |
| sessions | leased raw-pipe and probe-gated PTY modes with one Unix-WebSocket attachment |
| capsules | digest-verified read-only runtime material beside a writable workspace |
| distribution | Apache-2.0 source and a keyless-signed daemon image |

## Explicitly absent

- Git workspace sources and snapshots
- production network session authority
- workloads, images, volumes, and endpoints
- Docker and Kubernetes drivers
- cross-machine scheduling
- stable signed contract-bundle publication
- a production hosted trust envelope

Absent features are not stubs. They are missing from capability facts and answer with `unserved` or
a specific refusal when requested.

## Trust limitations

- The minimum host boundary does not claim protection from kernel compromise.
- A verified capsule identifies its provided bytes, not the entire host runtime closure.
- Static-bearer TCP is development-only and is not suitable for public or shared ingress.
- One daemon is one trust domain; it is not a multi-tenant isolation layer.
- Development bundle `0.11.0` declares v1 and v2 routes, the PTY session mode, storage quotas and
  metrics, while the daemon still advertises the
  deliberately older `substrate-wire/0.4.0` contract header.

## Reading status safely

The homepage describes durable product boundaries. This page carries the changing implementation
snapshot. For a real deployment, machine facts take precedence over both: they describe what that
specific daemon verified at runtime.

Start with [getting started](./getting-started.md), then read
[confinement and refusal](./concepts/confinement.md) before enabling execution.
