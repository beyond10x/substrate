---
format: aep.planning-md/1
id: story:production-tls-control-listener
kind: story
status: implemented
title: Production control traffic uses TLS
summary: A rustls HTTPS/WSS listener with explicit certificates replaces development plaintext for non-loopback deployments.
owner: substrate
tags:
- remote
- security
- tls
- wave/remote-foundation-01
relations:
- decomposes: epic:remote-serving
- depends_on: story:promote-development-contract-frontier
revision: 9
---
# Story: Production control traffic uses TLS

## Outcome

A remotely deployed daemon serves its HTTP and WebSocket control plane over TLS with explicit operator-provided identity and trust configuration.

## Design gate

An accepted design or ADR fixes certificate loading and rotation, client authentication mode, listener binding, forwarded-address treatment, WebSocket behavior, and the exact startup refusals before code. The Unix socket and kernel-peer-credential subject derivation remain unchanged.

## Acceptance

The daemon uses rustls and accepts no production plaintext listener. A non-loopback static-bearer configuration is refused at startup. Certificate and key files must be explicit, owner-safe and reloadable without exposing bytes in logs, events or metrics. Tests cover valid TLS, unknown CA, wrong server name, expired/not-yet-valid certificates, missing key material, plaintext attempts, WebSocket upgrade, rotation and shutdown. Development plaintext remains loopback-only and visibly marked development.

## Out of Scope

Public ingress controllers, ACME automation, service-mesh-specific configuration and hosted token verification.

## Design closure — 2026-09-01

- [ADR 0024](../../../adr/0024-production-network-control-uses-server-authenticated-tls.md) accepts the approved server-authenticated TLS posture: a distinct TLS 1.3 HTTPS/WSS listener, no mTLS principal mapping, explicit owner-safe key material, atomic SIGHUP rotation, no forwarded-address trust and named startup/reload refusals.
- Implementation remains sequenced after `story:promote-development-contract-frontier`; no listener code starts while that dependency is proposed.

## Implementation evidence — 2026-09-01

- `TlsDaemonConfig` and the CLI expose a distinct production listener with explicit bind, certificate-chain and private-key paths. The server enables rustls TLS 1.3 and HTTP/1.1 only; the development static-bearer listener now refuses every non-loopback address as `tls.listener-config-invalid`.
- Identity loading opens both paths with `O_NOFOLLOW`, bounds their bytes, requires regular non-empty files, requires the key to be owned by the effective uid with no group/other mode bits, validates every certificate time window, requires exactly one supported private key and proves that it matches the leaf before binding.
- SIGHUP parses and matches the pair as one replacement snapshot. Valid replacements affect only new connections; invalid replacements retain the prior snapshot and emit `tls.reload-invalid` without parser output or material bytes.
- Production HTTPS/WSS remains behind a fail-closed pre-admission response until the dependent hosted trust-envelope verifier derives a remote subject. Forwarded and caller-written identity headers are never consulted, and no certificate becomes a caller principal.
- `crates/substrate-daemon/tests/tls_listener.rs` drives the shipped daemon and proves valid TLS, TLS 1.3/HTTP/1.1 negotiation, unknown CA, wrong name, expired and not-yet-valid material, missing/unsafe/mismatched key refusal, plaintext receiving no HTTP response, WSS upgrade attempts over the authenticated connection, valid and invalid rotation, existing-connection snapshot retention and bounded SIGTERM shutdown.
- Public README, status, roadmap and deployment documentation distinguish the implemented transport from absent hosted admission. `npm ci`, `npm run typecheck` and `npm run build` pass; the rendered guide was inspected at 1440px and 390px widths.
- `bash scripts/gate.sh` completed with `gate: passed`, including release-mode workspace tests, strict Clippy, full-history secret scanning, RustSec/HTTP2 policy, deterministic third-party notices, package archives and all twelve frozen bundle checks.
- `bash scripts/delegated-lane.sh` also passed after the full gate: host-driver confinement, PTY, managed SDK, MCP cleanup and the shipped-daemon delegated runtime inventory remained green.
