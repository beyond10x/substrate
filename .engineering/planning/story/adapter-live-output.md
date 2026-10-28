---
format: aep.planning-md/3
id: story:adapter-live-output
kind: story
status: draft
title: A confinement adapter delivers a child's output while it runs
relations:
- decomposes: epic:confinement-contract-family
- depends_on: story:adapter-stdin-pipe
revision: 4
---
# A confinement adapter delivers a run's output while it runs

## Design

A consumer needs a child's output while it runs; today it is read after the run ends.
[Design 32](../../../docs/design/32-adapter-stdin-and-live-output.md) § 3, § 5, § 6 (live half),
§ 11 story 2, D3 to D7, D9, D12 (the `Live` refusal), D16. One unit: the spec commit first
(`output_live`, its outcome, open steps, eight scenarios, pinned counts), then the adapters. It
adds no adapter and no lane test file, so ADR 0044 decision 6 does not bind it (design § 6).

## Depends on

`story:adapter-stdin-pipe`: its spec commit states `output_live` beside `stdin_pipe`, and it
extends that story's `check_streams`, `run_with_stdin`, linux `Pipes` channel and `systemd`
forwarding. It absorbs `story:adapter-live-spec` (folded by review pass 3 H3).

## Acceptance

- Spec commit: `Fact` gains `output_live`, `AdmitProfile` `fact-unserved-output-live`, open steps
  as for `stdin_pipe`; `FACT_ORDER` is `[Fact; 13]`. Counts: `PORTABLE_DECLARED` 20,
  `PORTABLE_EXECUTED` 19/20, `LINUX_DELEGATED` 11, `MACOS_DECLARED` 10, `MACOS_IO` 2;
  `contracts.md` holds 65 authored scenarios and `check-docs` passes; `spec/coverage.md` states
  65 authored scenarios and 37 outcomes.
- On a natural end each stream's chunks are in order, contiguous, equal to the record with no
  marker; `Bounded` when `*_truncated` and the delivered offset equals the record's length (T4, T6).
- An unread run stalls after 1 s and reads `Stalled` after its chunks; `output_stalled` is true
  (T5, T11). After `Kill` delivery stops with no event and the chunks are a prefix (T19).
- On `local` a child trapping `TERM` keeps delivering after `signal(Term)`, ends naturally, and
  its record equals the delivered bytes (T21).
- Under `linux`, a 30 s `read_output` on a silent child does not hold `signal(Kill)`, which
  returns within 3 s (T20).
- `check_streams` refuses `queued_chunks` 0 and 17 and `Live` with `Tail` (T1b); `run_with_stdin`
  refuses `Live` `confinement.live-output-unread` (T7b); `pod` refuses `Live` (T17); `systemd`
  around `linux` forwards `read_output` and `output_stalled` (T13).
- Every lane is green on the story's head before it merges; `macos.yml`'s pinned pass count moves.
