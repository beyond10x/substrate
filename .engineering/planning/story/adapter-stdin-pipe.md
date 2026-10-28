---
format: aep.planning-md/3
id: story:adapter-stdin-pipe
kind: story
status: draft
title: A confinement adapter feeds a child's standard input
relations:
- decomposes: epic:confinement-contract-family
revision: 1
---
# A confinement adapter feeds a child's standard input

## Consumer ask

A consumer launching children through the confinement adapters (loom,
`story:child-launches-through-confinement-adapters`) needs to write the child's standard input.
Today every adapter runs the child with standard input null.

## Design

[Design 32](../../../docs/design/32-adapter-stdin-and-live-output.md) D1, D2, D7, D8, D9, D10:
a profile field `StdinMode::Piped { limit_bytes }`, `write_stdin` and `close_stdin` on the adapter
trait, the `run_with_stdin` helper, the `stdin_pipe` fact, the session bounds (1 B to 16 MiB total,
1 B to 64 KiB per write, ended by EOF) and the `session.*` refusal codes.

## Acceptance

- An adapter that declares `stdin_pipe` delivers every byte written, in order, and the child reads
  EOF after `close_stdin`; a write past `limit_bytes` is refused with the session code and the
  child sees nothing of it.
- An adapter without the fact refuses a `Piped` profile `confinement.fact-unserved` naming
  `stdin_pipe` before anything runs; `pod` is one of them.
- Wave 1 (`local`, `linux`, `systemd`) lands with its ESS confinement scenarios in the same change;
  `macos` and `oci` follow only with their lane case green.
- Each new case runs once in its existing lane test file; no new CI job.
