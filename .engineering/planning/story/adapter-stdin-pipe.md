---
format: aep.planning-md/3
id: story:adapter-stdin-pipe
kind: story
status: active
title: A confinement adapter feeds a child's standard input
relations:
- decomposes: epic:confinement-contract-family
revision: 7
transitions:
- {from: "draft", to: "proposed", at: "2026-10-28T20:25:25Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"review_outcome":2}}}
- {from: "proposed", to: "active", at: "2026-10-28T20:25:25Z", actor: "human:timo", revision: 7, decided_on: {"recorded":{"review_outcome":2}}}
---
# A confinement adapter feeds a child's standard input

## Design

A consumer needs to write a child's standard input; today every adapter gives it `/dev/null`.
[Design 32](../../../docs/design/32-adapter-stdin-and-live-output.md) § 2, § 5, § 6 (stdin half),
§ 11 story 1, D1, D2, D7, D9, D12 (without the `Live` refusal), D13, D15. One unit: the spec commit
first (`stdin_pipe`, its outcome, open steps, eight scenarios, pinned counts), then the adapters.
A spec author writes the spec commit; the adapter implementor never edits the suite (ADR 0044 decision 6, design § 6).

## Depends on

None. It absorbs `story:adapter-io-spec` (folded by review pass 3 H3).

## Acceptance

- Spec commit: `Fact` gains `stdin_pipe`, `AdmitProfile` `fact-unserved-stdin-pipe`; every open
  step stating `local`, `linux`, `systemd`, `macos` or `oci` names it, the `pod` set unchanged;
  `cargo xtask check-ess` regenerates the suite and `Fact`, and `FACT_ORDER` is `[Fact; 12]`.
- Counts move as § 6 states: `PORTABLE_DECLARED` 19, `PORTABLE_EXECUTED` 18/19,
  `LINUX_DELEGATED` 10, `MACOS_DECLARED` 9, new `MACOS_IO` 1; `contracts.md` holds 57 authored
  scenarios and its lanes table, which `check-docs` passes; `spec/coverage.md` states 57
  authored scenarios and 36 outcomes.
- Every byte written arrives in order and the child reads EOF after `close_stdin`; out-of-range
  limits and writes get § 2's session codes (T1a, T2, T3).
- A cancelled write closes stdin, under `linux` through `Handle::try_current`; the next write is
  `session.input-closed`, the child reads a prefix then EOF; the trimmed `wc` count holds (T8).
- `close_stdin` waits for a pending write (T9). From the observed end a write is
  `session.input-closed`; one pending then is `session.write-failed`, a `setsid` descendant
  holding descriptor 0 included (T10 a to c).
- `run_with_stdin` refuses `Null` and an over-limit input before start; a dropped helper kills its
  run (T7a). `pod`, and `oci` outside `OCI_STDIN_PINNED`, refuse `Piped` naming `stdin_pipe` (T16,
  T17); `oci` proves every byte and EOF (T15); `systemd` around `linux` forwards both (T13).
- Every lane is green on the story's head before it merges; `macos.yml`'s pinned pass count moves.
