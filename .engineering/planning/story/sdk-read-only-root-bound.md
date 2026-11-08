---
format: aep.planning-md/3
id: story:sdk-read-only-root-bound
kind: story
status: draft
title: The Rust SDK bounds read-only roots before sending
summary: CommandBuilder, pipe_session and pty_session refuse more than MAX_READ_ONLY_ROOTS read-only roots before the request is sent, as they do writable roots.
owner: substrate
tags:
- sdk
relations:
- decomposes: epic:rust-sdk
revision: 1
---
# Story: The Rust SDK bounds read-only roots before sending

## Outcome

A client of the Rust SDK that names more read-only roots than one start may hold on a pipe session,
a PTY session or an exec is refused by the SDK before the request is sent, as it is for writable
roots. The wire has bounded read-only roots at `MAX_READ_ONLY_ROOTS` since ADR 0010; only the SDK
builders send whatever list they are given.

## Why

`CommandBuilder::dispatch` and `PipeSessionBuilder::start` (crates/b10x-substrate-sdk/src/lib.rs)
check the workspace access and the scratch quota before sending, and `story:sdk-writable-roots`
adds the writable-root bound beside them; neither checks the read-only-root count, so an
over-bound list costs a round trip and arrives as the daemon's refusal instead of the SDK's.

## Acceptance

- `CommandBuilder` and `PipeSessionBuilder` (and so `pty_session`) refuse more than
  `substrate_wire::MAX_READ_ONLY_ROOTS` read-only roots with `SdkError::Protocol` carrying
  `WireValidationError::InvalidReadOnlyRootBounds`, before any request is sent.
- A unit test against a fake daemon shows both refusals and that the daemon's next request after
  them is an unrelated sentinel lookup.
- Every other rule on a read-only root stays the daemon's, answered with its named refusal.
- No wire, bundle or daemon change.

## Boundaries

- The path, reserved-mount, duplicate and overlap rules of `validate_read_only_roots` are not
  moved into the SDK.
- No new re-export: the SDK's export list is left to `story:sdk-writable-roots`, which rewrites it.
