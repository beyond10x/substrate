---
format: aep.planning-md/3
id: story:sdk-reexports-refusal-codes
kind: story
status: draft
title: The Rust SDK re-exports the wire's refusal codes
summary: Clients compare refusal codes such as SESSION_OUTPUT_LIMIT against SDK exports, and the SDK docs say a session's refusal is read from exec_observation().
owner: substrate
tags:
- sdk
relations:
- decomposes: epic:rust-sdk
revision: 1
---
# Story: The Rust SDK re-exports the wire's refusal codes

## Outcome

A client of the Rust SDK compares a refusal's code against a constant the SDK exports, such as
`SESSION_OUTPUT_LIMIT` or `EXEC_WRITABLE_ROOTS_HOST_UNSERVED`, and does not write the string
itself. The SDK's documentation says where each session ending is read, so a client finds
`session.output-limit` without reading the daemon's source.

## Why

- The wire defines each code as a constant, for example `EXEC_WRITABLE_ROOTS_HOST_UNSERVED`
  (crates/substrate-wire/src/lib.rs:306) and `SESSION_OUTPUT_LIMIT`
  (crates/substrate-wire/src/lib.rs:440). The SDK re-exports wire types and bounds but no code
  constant (crates/b10x-substrate-sdk/src/lib.rs:43-56).
- So clients write literals. The SDK's own test does too: `"exec.output-unrecorded"` and
  `"session.output-limit"` (crates/b10x-substrate-sdk/tests/unrecorded.rs:318, :328). A client
  could depend on the wire crate itself, but the SDK pins it to an exact version
  (crates/b10x-substrate-sdk/Cargo.toml:38), so a second pin has to move in step with the SDK.
- `PipeSessionObservation` has no refusal field (crates/b10x-substrate-sdk/src/model.rs:369-384),
  and neither does the wire's `PipeSession` (crates/substrate-wire/src/lib.rs:3300-3315). The
  refusal is on the exec by design: the host records `session.output-limit` there
  (crates/substrate-host/src/process.rs:3400-3415). The SDK reads it through
  `PipeSession::exec_observation` (crates/b10x-substrate-sdk/src/lib.rs:1483-1489) as
  `ExecObservation::refusal` (crates/b10x-substrate-sdk/src/model.rs:288). Nothing on
  `PipeSessionObservation` or `PipeSession` points there, so a consumer concluded the ending
  cannot be read.

## Acceptance

- The SDK re-exports every refusal-code constant of `substrate_wire` that a daemon refusal or an
  `ExecRefusal` can carry. A test fails when the wire gains such a constant and the SDK does not
  export it.
- The SDK's tests compare against the exported constants, not literals
  (crates/b10x-substrate-sdk/tests/unrecorded.rs:318, :328 and the like).
- The rustdoc of `PipeSessionObservation`, `PipeSession::refresh` and
  `PipeSession::exec_observation` says that a session's refusal, `session.output-limit` included,
  is read from `exec_observation().refusal`.
- No wire, bundle or daemon change.

## Boundaries

- No refusal field on `PipeSessionObservation`. Design 13 put the refusal on the exec
  (docs/design/13-pty-sessions.md:83-86), and moving it is a wire change.
- `story:sdk-writable-roots` also edits the SDK's re-export block
  (crates/b10x-substrate-sdk/src/lib.rs:43-56); this story lands after it or merges with it.
