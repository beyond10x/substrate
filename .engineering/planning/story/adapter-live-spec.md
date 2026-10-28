---
format: aep.planning-md/3
id: story:adapter-live-spec
kind: story
status: archived
title: The confinement specification states adapter live output
relations:
- decomposes: epic:confinement-contract-family
revision: 3
transitions:
- {from: "draft", to: "archived", at: "2026-10-28T19:24:54Z", actor: "human:timo", revision: 3}
---
# The confinement contract states live output

## Design

[Design 32](../../../docs/design/32-adapter-stdin-and-live-output.md) § 6 (live half), § 11
story 3, D7 and D9: the spec change for `output_live` only, made by a spec story because ADR 0044
decision 6 keeps adapter stories out of the suite. No adapter code and no adapter proof.

## Depends on

`story:adapter-stdin-pipe`: the stdin pair has merged, so this story's open steps state
`stdin_pipe` and add `output_live` beside it.

## Acceptance

- `spec/confinement/confinement.yaml`: `Fact` gains `output_live` after `stdin_pipe`;
  `AdmitProfile` gains `fact-unserved-output-live`; the declared sets above `OpenAdapter` state
  § 5's `output_live` with its source, the `pod` set unchanged.
- Every open step that states the declared set of `local`, `linux`, `systemd`, `macos` or `oci`
  names `output_live`, and the eight live-output scenarios of § 6's table exist.
- `cargo xtask check-ess` regenerates `spec/confinement-suite.json` and `Fact`; `FACT_ORDER` is
  `[Fact; 13]` and the crate compiles; the public-items and `check-docs` fact-table lines move.
- The pinned counts move as § 6's table states: `PORTABLE_DECLARED` 20, `PORTABLE_EXECUTED` 19/20,
  `LINUX_DELEGATED` 11, `MACOS_DECLARED` 10, `MACOS_IO` 2; the README states portable 20,
  delegated 20, macos 10, oci 7, pod 5, interpreted 3 and the obligation counts `check-ess`
  reports.
- It lands with `story:adapter-live-output` as one unit, merged only with every lane green on the
  unit's head. `pod-output-live-unserved-on-pod` passes from this story's commit on.

Folded by review pass 3 H3 of design 32: a spec story whose acceptance needs its adapter story
cannot merge alone under one story per unit, so this spec change moved into its adapter story as
that story's first commit, and this story is archived.
