---
title: Status and limitations
description: What the current Substrate development release serves, and what remains absent.
---

# Development release, not a stable published contract

Substrate has a tagged, keyless-signed daemon image and a public, keyless-signed, digest-pinned
development contract bundle. Distribution is proven; the bundle remains a development contract
because stability is a separate compatibility decision.

The current implementation is a Linux host slice. Treat every capability as deployment-specific and
verify it through `GET /v1/machine`.

## Served now

| Area | Current state |
|---|---|
| personal transport | owner-permissioned Unix socket with kernel-derived local identity |
| production network transport | TLS 1.3 HTTPS/WSS with explicit identity files, atomic SIGHUP rotation and five-minute exact-audience Identity admission resolved before every route |
| workspaces | empty source, guarded file access, atomic replacement, destruction, leases |
| exec | available only with the complete probed Linux confinement floor |
| durability | operation reservation before dispatch, persisted terminal observations and output |
| recovery | operation lookup, bounded events, reconciliation snapshots, restart-to-unknown |
| limits | body, path, output, retention, concurrency, process, memory, CPU, duration, and probe-gated writable-storage bounds |
| observations | explicit live and terminal wall, CPU, memory peak/current, process, OOM, block-I/O, and scratch usage facts |
| sessions | leased raw-pipe and probe-gated PTY modes at `/v1/sessions`, with one Unix-WebSocket or authority-bound hosted WSS attachment |
| Rust SDK source | complete promoted-contract coverage over Unix or explicit-root TLS 1.3 HTTPS/WSS, with serializable observations, exact optional facts, keyed mutations, PTY resize, metrics and snapshots |
| disposable MCP | private stdio adapter over the SDK; bounded tools/resources, caller operation IDs, portable named refusal and native delegated cleanup evidence; published as a separately signed 0.5.0 image |
| capsules | digest-verified read-only runtime material beside a writable workspace |
| distribution | Apache-2.0 source plus keyless-signed 0.5.0 daemon and MCP images and the signed 0.15.0 development bundle |

## Explicitly absent

- Git workspace sources and snapshots
- workloads, images, volumes, and endpoints
- Docker and Kubernetes drivers
- cross-machine scheduling
- stable signed contract-bundle publication

Absent features are not stubs. They are missing from capability facts and answer with `unserved` or
a specific refusal when requested.

## Trust limitations

- The minimum host boundary does not claim protection from kernel compromise.
- A verified capsule identifies its provided bytes, not the entire host runtime closure.
- Static-bearer TCP is loopback-only, development-only, and unsuitable for public or shared ingress.
- Production TLS admits callers only through online Identity resolution. Identity unavailability
  fails closed, and no cached authority or caller-written identity is used.
- One daemon is one trust domain; it is not a multi-tenant isolation layer.
- Tagged release `0.5.0` advertises `substrate-wire/0.15.0`; current development source advertises
  additive `substrate-wire/0.16.0` and the SHA-256 of its inner `bundle.json` as one claim. Version
  `0.15.0` deliberately replaces
  `/v1/pipe-sessions` with `/v1/sessions` and serves no compatibility alias. The published bundle
  remains a development contract.

## Reading status safely

The homepage describes durable product boundaries. This page carries the changing implementation
snapshot. For a real deployment, machine facts take precedence over both: they describe what that
specific daemon verified at runtime.

Start with [getting started](./getting-started.md), then read
[confinement and refusal](./concepts/confinement.md) before enabling execution.
