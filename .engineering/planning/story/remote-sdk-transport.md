---
format: aep.planning-md/1
id: story:remote-sdk-transport
kind: story
status: active
title: The Rust SDK connects over HTTPS and WSS
summary: Explicit roots, server identity and an asynchronous token provider extend the SDK without changing its Unix-socket behavior.
owner: substrate
tags:
- remote
- sdk
- tls
- wave/remote-foundation-01
relations:
- decomposes: epic:remote-serving
- depends_on: story:production-tls-control-listener
- depends_on: story:hosted-trust-envelope-admission
- depends_on: story:sdk-promoted-contract-parity
revision: 4
---
# Story: The Rust SDK connects over HTTPS and WSS

## Outcome

A Rust service can address a remote Substrate instance with the same typed workspace, operation, execution, event and session handles used over a Unix socket.

## Acceptance

The builder requires an explicit HTTPS endpoint, trust roots and expected server identity; it never disables verification. An asynchronous token provider supplies short-lived authority per request and can refresh after an authorization failure without replaying a completed mutation. HTTP and WSS share TLS/auth configuration. Operation IDs survive timeouts, disconnects and token refresh. Tests cover unknown roots, name mismatch, expired authority, rotation, lost mutation responses, event gaps and WebSocket reconnect requiring fresh session authority. Unix and managed-daemon modes remain source-compatible.

## Out of Scope

Credential storage, Identity login flows, load balancing, automatic fleet selection and policy defaults.
