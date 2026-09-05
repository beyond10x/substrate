---
title: Status and limitations
description: What the current Substrate development release serves, and what remains absent.
---

# What is available today?

Substrate has a tagged, keyless-signed daemon image and a public, keyless-signed, digest-pinned
development contract bundle. Distribution is proven; the bundle remains a development contract
because stability is a separate compatibility decision.

Release [0.7.3](https://github.com/beyond10x/substrate/releases/tag/0.7.3), published on
5 September 2026, ships the daemon and disposable MCP images with the signed
`substrate-wire/0.16.0` development bundle.

The current implementation is a Linux host slice. Treat every capability as deployment-specific and
verify it through `GET /v1/machine`.

## Served now

| Area | Current state |
|---|---|
| personal transport | owner-permissioned Unix socket with kernel-derived local identity |
| production network transport | TLS 1.3 HTTPS/WSS with explicit identity files, atomic SIGHUP rotation and five-minute exact-audience Identity admission resolved before every route |
| workspaces | empty or configured, authorized HTTPS Git source; exact-commit materialization, guarded files, bounded Git observations, destruction and leases |
| exec | available only with the complete probed Linux confinement floor |
| durability | operation reservation before dispatch, persisted terminal observations and output |
| recovery | operation lookup, bounded events, reconciliation snapshots, restart-to-unknown |
| limits | body, path, output, retention, concurrency, process, memory, CPU, duration, and probe-gated writable-storage bounds |
| observations | explicit live and terminal wall, CPU, memory peak/current, process, OOM, block-I/O, and scratch usage facts |
| sessions | leased raw-pipe and probe-gated PTY modes at `/v1/sessions`, with one Unix-WebSocket or authority-bound hosted WSS attachment |
| Rust SDK source | complete promoted-contract coverage over Unix or explicit-root TLS 1.3 HTTPS/WSS, with serializable observations, exact optional facts, keyed mutations, PTY resize, metrics and snapshots |
| disposable MCP | private stdio adapter over the SDK; bounded tools/resources, caller operation IDs, portable named refusal and native delegated cleanup evidence; published as a separately signed 0.7.3 image |
| capsules | digest-verified read-only runtime material beside a writable workspace |
| distribution | Apache-2.0 source plus keyless-signed 0.7.3 daemon and MCP images and the signed 0.16.0 development bundle |

## Explicitly absent

- general workspace backup/restore snapshots (reconciliation snapshots recover observed resource state)
- workloads, images, volumes, and endpoints
- Docker and Kubernetes drivers
- cross-machine scheduling
- stable signed contract-bundle publication

Unsupported capabilities on existing routes produce `unserved` or a specific refusal. Future
resource families have no implemented routes; unknown addresses return HTTP 404 with `resource.not-found`. Their names
in a broader architecture do not imply served endpoints.

## Trust limitations

- The minimum host boundary does not claim protection from kernel compromise.
- A verified capsule identifies its provided bytes, not the entire host runtime closure.
- Static-bearer TCP is loopback-only, development-only, and unsuitable for public or shared ingress.
- Production TLS admits callers only through online Identity resolution. Identity unavailability
  fails closed, and no cached authority or caller-written identity is used.
- One daemon is one trust domain; it is not a multi-tenant isolation layer.
- Release `0.7.3` advertises `substrate-wire/0.16.0` and the SHA-256 of its inner
  `bundle.json` as one claim. The outer OCI package has its own digest. Version `0.15.0`
  deliberately replaced `/v1/pipe-sessions` with `/v1/sessions` and serves no compatibility alias.
- Git materialization needs a configured source and transient Connector authority. The conditional
  `workspace.git` fact does not authorize arbitrary remote URLs.
- Sessions publish operation-ledger events; a separate public `session.*` vocabulary is absent.
  See [model coverage and derivation](./concepts/model.md).


## Reading status safely

The homepage describes durable product boundaries. This page carries the changing implementation
snapshot. For a real deployment, machine facts take precedence over both: they describe what that
specific daemon verified at runtime.

Start with [getting started](./getting-started.md), then read
[confinement and refusal](./concepts/confinement.md) before enabling execution.
