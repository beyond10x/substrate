---
format: aep.planning-md/1
id: story:rust-sdk-managed-daemon
kind: story
status: draft
title: The Rust SDK owns a private daemon child
summary: External and linked-current-executable builders start, verify, stop and reap a separate daemon process.
owner: substrate
tags:
- sdk
relations:
- decomposes: epic:rust-sdk
- depends_on: story:rust-sdk-client
revision: 1
---
# Story: The Rust SDK owns a private daemon child

## Outcome

A caller supplies a durable data directory and deployment id, then receives a client connected to a private daemon child. External-binary and opt-in linked-current-executable modes have the same socket contract and lifecycle.

## Acceptance

The child has a separate pid, admits only the invoking effective uid, proves readiness through `/v1/machine`, retains durable data, stops on explicit shutdown or owner loss, and is force-killed and reaped after a bounded grace period. Linked mode re-executes the application; it never executes a request in process.
