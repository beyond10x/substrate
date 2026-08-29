---
title: Status and limitations
description: What the current Substrate development release serves, and what remains absent.
---

# Development release, not a stable published contract

Substrate has a tagged repository release and reproducible development contract bundles. Stable
publication still requires complete packaging, signing, and digest pinning.

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
| limits | body, path, output, retention, concurrency, process, memory, CPU, and duration bounds |
| sessions | leased raw-pipe development slice with one Unix-WebSocket attachment |
| capsules | digest-verified read-only runtime material beside a writable workspace |

## Explicitly absent

- Git workspace sources and snapshots
- PTY sessions
- production network session authority
- workloads, images, volumes, and endpoints
- Docker and Kubernetes drivers
- cross-machine scheduling
- stable signed runtime packaging
- a production hosted trust envelope

Absent features are not stubs. They are missing from capability facts and answer with `unserved` or
a specific refusal when requested.

## Trust limitations

- The minimum host boundary does not claim protection from kernel compromise.
- A verified capsule identifies its provided bytes, not the entire host runtime closure.
- Static-bearer TCP is development-only and is not suitable for public or shared ingress.
- One daemon is one trust domain; it is not a multi-tenant isolation layer.

## Reading status safely

The homepage describes durable product boundaries. This page carries the changing implementation
snapshot. For a real deployment, machine facts take precedence over both: they describe what that
specific daemon verified at runtime.

Start with [getting started](./getting-started.md), then read
[confinement and refusal](./concepts/confinement.md) before enabling execution.
