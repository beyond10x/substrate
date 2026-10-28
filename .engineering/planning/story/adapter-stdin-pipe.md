---
format: aep.planning-md/3
id: story:adapter-stdin-pipe
kind: story
status: draft
title: A confinement adapter feeds a child's standard input
relations:
- decomposes: epic:confinement-contract-family
- depends_on: story:adapter-io-spec
revision: 3
---
# A confinement adapter feeds a child's standard input

## Design

A consumer needs to write a child's standard input; today every adapter gives it `/dev/null`.
[Design 32](../../../docs/design/32-adapter-stdin-and-live-output.md) § 2, § 5, § 11 story 2, D1,
D2, D12 (without the `Live` refusal), D13, D15: `StdinMode::Piped { limit_bytes }`, the shared
`StdinWriter`, `write_stdin`, `close_stdin`, `run_with_stdin`, `check_streams` with the
`limit_bytes` check, and `stdin_pipe` on `local`, `linux`, `systemd` around both, `macos` and `oci`.

## Depends on

`story:adapter-io-spec`. Lands with it as one unit. Edits no scenario and no pinned scenario count.

## Acceptance

- An adapter that declares `stdin_pipe` delivers every byte written, in order, and the child reads
  EOF after `close_stdin`; out-of-range limits and writes get the session codes of § 2 (T1a, T2, T3).
- A cancelled write closes stdin, under `linux` through `Handle::try_current`; the next write is
  `session.input-closed` and the child reads a prefix, then EOF (T8, driven by the fill and the gate).
- `close_stdin` waits for a pending write (T9). From the observed run end a write is
  `session.input-closed`; a write pending then is `session.write-failed`, a `setsid` descendant
  holding descriptor 0 included (T10 a to c).
- `run_with_stdin` refuses `Null` and an over-limit input before start, and a dropped helper kills
  its run (T7a).
- `pod`, and `oci` outside `OCI_STDIN_PINNED`, refuse `Piped` naming `stdin_pipe` (T16, T17); `oci`
  proves every byte and EOF through the runtime client (T15); `systemd` around `linux` forwards both
  methods (T13).
- Every lane is green on the unit's head; macos cases move `macos.yml`'s pinned pass count.
