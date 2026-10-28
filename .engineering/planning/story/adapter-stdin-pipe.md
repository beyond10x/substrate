---
format: aep.planning-md/3
id: story:adapter-stdin-pipe
kind: story
status: draft
title: A confinement adapter feeds a child's standard input
relations:
- decomposes: epic:confinement-contract-family
- depends_on: story:adapter-io-spec
revision: 2
---
# A confinement adapter feeds a child's standard input

## Consumer ask

A consumer launching children through the confinement adapters needs to write the child's
standard input. Today every adapter runs the child with standard input null.

## Design

[Design 32](../../../docs/design/32-adapter-stdin-and-live-output.md) D1, D2, D7, D8, D9, D10,
D12, D13, D14: a profile field `StdinMode::Piped { limit_bytes }`, the shared `StdinWriter`,
`write_stdin` and `close_stdin` on the adapter trait, the `run_with_stdin` helper, the `stdin_pipe`
fact, the session bounds (1 B to 16 MiB total, 1 B to 64 KiB per write, ended by EOF) and the
`session.*` refusal codes.

## Depends on

`story:adapter-io-spec`, which makes every spec change of the design first. This story edits no
scenario and not the suite (ADR 0044 decision 6).

## Acceptance

- An adapter that declares `stdin_pipe` delivers every byte written, in order, and the child reads
  EOF after `close_stdin`; a write past `limit_bytes` is refused with the session code and the
  child sees nothing of it.
- A cancelled write closes stdin: the next write is `session.input-closed` and the child reads a
  prefix, then EOF. `close_stdin` waits for a pending write; a write pending when the run ends is
  `session.write-failed`.
- `run_with_stdin` refuses a `Live` profile `confinement.live-output-unread` before anything runs.
- An adapter without the fact refuses a `Piped` profile `confinement.fact-unserved` naming
  `stdin_pipe` before anything runs: `pod`, and `oci` on a runtime and version pair outside
  `OCI_STDIN_PINNED`.
- Served on `local`, `linux`, `systemd` around both, `macos` and `oci`; the lanes the spec story
  turned red for this fact are green on the integration branch once this story and
  `story:adapter-live-output` have landed.
- Each new case runs once in its existing lane test file; no new CI job.
