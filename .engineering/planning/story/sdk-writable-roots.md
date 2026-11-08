---
format: aep.planning-md/3
id: story:sdk-writable-roots
kind: story
status: active
title: The Rust SDK's session and exec builders carry writable roots
summary: PipeSessionBuilder, pty_session and CommandBuilder (the exec builder) name writable host roots, as the wire has carried since bundle 0.22.0.
owner: substrate
tags:
- sdk
relations:
- decomposes: epic:rust-sdk
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-08T18:45:46Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-08T18:45:46Z", actor: "human:timo", revision: 3}
---
# Story: The Rust SDK's session and exec builders carry writable roots

## Outcome

A client of the Rust SDK names a writable host root on a pipe session, a PTY session or an exec,
as it already names a read-only root, and the daemon mounts it writable in the confined run. The
wire has carried `writable_roots` since bundle 0.22.0 and the daemon serves `--writable-root`;
only the SDK builders send an empty list.

## Why

`story:sdk-promoted-contract-parity` covered read-only roots and secret slots and left writable
roots out, so `PipeSessionBuilder::start` and `CommandBuilder::dispatch` set `writable_roots:
Vec::new()` (crates/b10x-substrate-sdk/src/lib.rs, in both builders). A consumer that runs its
agents in a box needs a host directory both the box and the host write: a terminal log, an
envelope directory the host delivers into.

## Acceptance

- `PipeSessionBuilder` (and so `pty_session`) and `CommandBuilder` each gain
  `writable_root(substrate_wire::WritableRoot) -> Self`, repeatable, mirroring `read_only_root`;
  the roots reach the start request in the order given.
- A public-SDK test against the daemon starts a session and an exec naming a declared writable
  root, writes a file in the run, and reads it on the host; a root the daemon does not declare is
  refused with the daemon's existing error, unchanged.
- More roots than `MAX_WRITABLE_ROOTS` are refused by the SDK before the request is sent, as the
  read-only roots' bound is.
- No wire, bundle or daemon change; the SDK's documentation names the new setters.

## Boundaries

- No Unix sockets inside the box (seccomp refuses `socket(AF_UNIX)`; the draft on the
  `adr/workspace-unix-sockets` branch owns that).
- No subpath of a declared root: the operator declares each root on the daemon.
