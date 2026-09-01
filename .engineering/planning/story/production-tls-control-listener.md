---
format: aep.planning-md/1
id: story:production-tls-control-listener
kind: story
status: proposed
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
revision: 4
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
