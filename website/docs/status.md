---
title: Status and limitations
description: What the current Substrate development release serves, and what remains absent.
---

# What is available today?

Substrate has a tagged, keyless-signed daemon image and a public, keyless-signed, digest-pinned
development contract bundle. Distribution is proven; the bundle remains a development contract
because stability is a separate compatibility decision.

Release [0.15.0](https://github.com/beyond10x/substrate/releases/tag/0.15.0), published on
29 October 2026, ships the daemon and disposable MCP images, the Helm chart and the signed
`substrate-wire/0.24.0` development bundle. [Release notes](./releases.md) lists what changed.
Release 0.16.0 is upcoming, not released.

The released implementation is a Linux host slice. Treat every capability as deployment-specific
and verify it through `GET /v1/machine`.

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
| disposable MCP | private stdio adapter over the SDK; bounded tools/resources, caller operation IDs, portable named refusal and native delegated cleanup evidence; published as a separately signed 0.15.0 image |
| confinement adapters | in process, through `substrate_host::confinement` and `b10x-substrate confine run`: `local`, `linux`, `macos`, `systemd`, `oci` and `pod`, each declaring its families and facts, with piped standard input and live output where the adapter proves them; see [confinement adapters](./concepts/adapters.md) |
| process events | one WebSocket stream of a host process's lifecycle and log lines |
| operational listener | opt-in `--ops-listen`: `/livez`, `/readyz` and OpenMetrics `/metrics` |
| Kubernetes serving | a signed Helm chart running one node-bound daemon per node |
| capsules | digest-verified read-only runtime material beside a writable workspace |
| distribution | Apache-2.0 source plus keyless-signed 0.15.0 daemon and MCP images, chart and binaries, and the signed 0.24.0 development bundle |

## Coming in 0.16.0

Implemented in source and not yet released:

| Area | State in source |
|---|---|
| Kubernetes namespace driver | `--driver kubernetes`: workspaces on a PersistentVolumeClaim in one namespace; a development profile |
| exec on a cluster | refused `exec.sandbox-unavailable`: no confinement fact is proved there yet |
| contract | development bundle `0.25.0`: the `kubernetes` driver word, the optional `confinement.no-egress` fact and three `501` refusals |
| executor image | `ghcr.io/beyond10x/b10x-substrate-executor`, published with the release |
| execution profile | AppArmor `substrate-host-exec-v2` is the default; nodes must load it |

The namespace driver's limits are in the
[deployment guide](./guides/deployment.md#kubernetes-namespace-driver).

## Explicitly absent

- general workspace backup/restore snapshots (reconciliation snapshots recover observed resource state)
- workloads, images, volumes, and endpoints
- a Docker driver behind the daemon, and any released cluster driver; the `oci` and `pod`
  confinement adapters run in the host crate, not behind the daemon
- confined exec through the Kubernetes namespace driver
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
  fails closed, and no cached authority or caller-written identity is used. Every request costs one
  Identity round trip on a reused keep-alive connection (at most 8 idle, closed after 30 s idle or
  any error), so a completed revocation applies to the next request.
- One daemon is one trust domain; it is not a multi-tenant isolation layer.
- Release `0.15.0` advertises `substrate-wire/0.24.0` and the SHA-256 of its inner
  `bundle.json` as one claim. The outer OCI package has its own digest. Bundle `0.15.0`
  deliberately replaced `/v1/pipe-sessions` with `/v1/sessions` and serves no compatibility alias.
- Git materialization needs a configured source and transient Connector authority. The conditional
  `workspace.git` fact does not authorize arbitrary remote URLs.
- Sessions publish operation-ledger events; a separate public `session.*` vocabulary is absent.
  See [model coverage and derivation](./concepts/model.md).


## Reading status safely

The homepage describes durable product boundaries. This page carries the changing implementation
snapshot. For a real deployment, machine facts take precedence over both: they describe what that
specific daemon verified at runtime.

Start with [getting started](./getting-started.md) or [a local daemon](./guides/run-a-command.md#start-with-an-observable-machine), then read
[confinement and refusal](./concepts/confinement.md) before enabling execution.
