---
format: aep.planning-md/1
id: epic:rust-sdk
kind: epic
status: implemented
title: Public Rust SDK
summary: A typed Rust client makes governed workspace and process execution straightforward without collapsing the daemon boundary.
owner: substrate
tags:
- rust
- sdk
revision: 6
---
# Epic: Public Rust SDK

## Outcome

Rust consumers create, control and observe confined workspaces and processes through an ergonomic, owner-released SDK. A managed local deployment is still a separate authenticated daemon child.

## Boundaries

The SDK serves the daemon-advertised `substrate-wire/0.4.0` surface over an owner-private Unix socket. It chooses no product execution policy, exposes no driver API, and does not make the development contract stable.

## Delivery

Typed client and recovery, managed daemon ownership, approved package publication, and public adoption documentation are separate stories under this epic.

## Reconciled state — 2026-09-01

Typed local and remote clients, managed daemon ownership, the public SDK journey and parity with the
daemon-advertised contract frontier are implemented. Registry publication was deliberately archived:
ADR 0030 replaces it with source consumption by local path or exact Git revision, and the repository
gate refuses every publishable workspace member. Remote HTTPS/WSS transport remains owned by
`epic:remote-serving`; its implementation does not keep this SDK epic open.
