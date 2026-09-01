---
format: aep.planning-md/1
id: story:sdk-promoted-contract-parity
kind: story
status: proposed
title: The Rust SDK covers the promoted contract
summary: Typed builders and observations cover the promoted PTY, confinement, quota and metrics surface without exposing driver internals.
owner: substrate
tags:
- contract
- sdk
- wave/remote-foundation-01
relations:
- decomposes: epic:rust-sdk
- depends_on: story:promote-development-contract-frontier
revision: 4
---
# Story: The Rust SDK covers the promoted contract

## Outcome

A Rust caller can use every supported operation and confinement option in the contract the daemon advertises, with typed requests, observations and refusals.

## Current overlap

The substrate-hardening worktree already adds SDK builders for workspace access, apertures, scratch, measurements, read-only roots, secret slots and capsules, plus exact applied and usage observations. That code is input to this story and must be reviewed or merged, not reimplemented. This story closes the remaining promoted-version and PTY/metrics parity after that work lands.

## Acceptance

The SDK supports PTY sessions and resize, workspace and scratch quotas, exact measurement selection and terminal usage, live metrics retrieval/streaming, workspace access, apertures, read-only roots, secret slots and execution capsules where the advertised bundle supports them. It preserves operation IDs across ambiguous transport outcomes and returns named refusals without collapsing them into strings. A clean-room journey uses only the published SDK API against the shipped daemon binary and asserts the promoted header. Driver names and host-library types never enter the public API.

## Out of Scope

Remote HTTPS transport, product policy helpers and automatic limit selection.

## Remote/MCP prerequisites — 2026-09-01

Before this story can close, the public SDK must additionally expose caller-supplied operation ids for file write/delete, workspace destroy and lease renewal, exec signal/retire and lease renewal; bounded exec-output paging; metrics retrieval; serializable SDK-owned observations or an exact projector; capability facts that preserve absence instead of collapsing it to `false`; and SDK-owned public bounds for generated schemas.

Managed-daemon shutdown must retain child and temporary-root ownership across kill/wait errors. A red test proves that unknown child absence never drops the temporary state, and startup cleanup observes the same rule. The daemon/adapter cleanup path must also prove no detached exec supervisor, workload process or cgroup survives orderly shutdown.
