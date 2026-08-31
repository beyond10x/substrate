---
format: aep.planning-md/1
id: epic:rust-sdk
kind: epic
status: active
title: Public Rust SDK
summary: A typed Rust client makes governed workspace and process execution straightforward without collapsing the daemon boundary.
owner: substrate
tags:
- rust
- sdk
revision: 4
---
# Epic: Public Rust SDK

## Outcome

Rust consumers create, control and observe confined workspaces and processes through an ergonomic, owner-released SDK. A managed local deployment is still a separate authenticated daemon child.

## Boundaries

The SDK serves the daemon-advertised `substrate-wire/0.4.0` surface over an owner-private Unix socket. It chooses no product execution policy, exposes no driver API, and does not make the development contract stable.

## Delivery

Typed client and recovery, managed daemon ownership, approved package publication, and public adoption documentation are separate stories under this epic.

## Reconciled state — 2026-09-01

Typed local client handles, managed daemon ownership and the public SDK journey are implemented. The epic remains active for registry publication evidence and parity with the contract frontier the daemon eventually advertises. Remote HTTPS/WSS transport is deliberately owned by epic:remote-serving so the local SDK boundary is not confused with hosted authentication.
