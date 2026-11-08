---
format: aep.planning-md/3
id: story:sdk-confinement-requires
kind: story
status: draft
title: The Rust SDK's session and exec builders carry sandbox.requires
summary: PipeSessionBuilder, pty_session and CommandBuilder name required confinement families and properties, which the daemon has checked since bundle 0.26.0.
owner: substrate
tags:
- sdk
relations:
- decomposes: epic:rust-sdk
- informed_by: story:confinement-adapter-on-the-wire
revision: 1
---
# Story: The Rust SDK's session and exec builders carry sandbox.requires

## Outcome

A client of the Rust SDK names the confinement families and properties a pipe session, a PTY
session or an exec requires. The daemon checks them at start and refuses an unserved one by name
before dispatch. Today the SDK always sends `requires: None`, so the daemon never holds a start to
what the client needs.

## Why

- Bundle 0.26.0 put `sandbox.requires` on both starts (ADR 0048 D4,
  adr/0048-the-wire-states-what-a-confinement-adapter-proves-never-its-name.md:48-53). The wire
  carries it (`ConfinementRequest::requires`, crates/substrate-wire/src/lib.rs:1631-1636; the
  `ConfinementRequires` type and its lexical check at :3742-3800). The daemon checks it right after
  `exec.capability-stale` (crates/substrate-daemon/src/app/operations.rs:461).
- `story:confinement-adapter-on-the-wire` (implemented) shipped that wire and daemon half. The SDK
  builders still hard-code `requires: None`: `PipeSessionBuilder::start`
  (crates/b10x-substrate-sdk/src/lib.rs:1404) and `CommandBuilder::dispatch` (:1848).
- So a consumer checks its required facts against the capability snapshot itself. Between that
  read and the start, nothing binds the start to the facts. The daemon's refusal and its ledger
  record (`confinement.family-unserved` and `confinement.fact-unserved`, 501) never happen for an
  SDK client.

## Acceptance

- `PipeSessionBuilder` (and so `pty_session`) and `CommandBuilder` each gain
  `requires(substrate_wire::ConfinementRequires) -> Self`. The value reaches
  `sandbox.requires` unchanged. Without the call the start sends no member, so its bytes and hash
  are unchanged.
- The SDK refuses an empty or duplicated requirement before sending, with
  `substrate_wire::validate_confinement_requires` (crates/substrate-wire/src/lib.rs:3780).
- A public-SDK test against the daemon:
  - a session and an exec requiring a family the daemon serves start, and the applied record
    echoes `requires`;
  - a requirement naming `network-ingress` is refused `confinement.family-unserved`, and the
    refusal surfaces through `SdkError` with its code and is recorded as refused;
  - a requirement naming an unproved property is refused `confinement.fact-unserved`.
- No wire, bundle or daemon change. The SDK's documentation names the new setter.

## Boundaries

- No `b10x-substrate` CLI flag for `requires`. ADR 0048 leaves that undecided (line 97).
- Writable roots are `story:sdk-writable-roots`.
