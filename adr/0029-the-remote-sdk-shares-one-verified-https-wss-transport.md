---
status: accepted
date: 2026-09-01
---

# ADR 0029: the remote SDK shares one verified HTTPS/WSS transport

## Context

ADR 0022 makes the Rust SDK a wire client but closes only its Unix-socket transport. ADRs 0024,
0026 and 0027 now provide the production TLS listener, hosted Identity admission and network
session authority which a remote client must consume together.

## Decision

The SDK adds a remote mode requiring an exact HTTPS endpoint, explicit PEM trust roots, an expected
DNS server identity and an asynchronous opaque-token provider. HTTP and WebSocket calls share that
immutable TLS and authentication configuration. Certificate verification cannot be disabled and
the SDK does not use environment proxies, redirects, ambient roots or credential storage.

The provider supplies authority per request and may be asked once to refresh after a named hosted
authentication 401. The original bytes and operation id are retained. Ambiguous transport outcomes
still use the operation ledger before any replay and never mint a replacement operation id.

Remote session attachment mints a one-use authority with a fresh ephemeral Ed25519 key and proves
the exact WSS TLS exporter on the upgrade. It never reuses the authority or represents a dropped
attachment as resumable. Unix attachment remains kernel-authorized and unchanged.

## Consequences

Rust services can use the same typed handles locally and remotely without learning the hosted
protocol. Remote configuration is deliberately more explicit than common HTTP clients because its
roots, name, bearer and channel proof are confinement authority. This decision changes only the
pre-1.0 SDK API; the advertised wire contract and frozen bundles do not change. The complete API,
retry and evidence closure is recorded in
[design 20](../docs/design/20-remote-sdk-transport.md).
