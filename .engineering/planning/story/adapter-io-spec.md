---
format: aep.planning-md/3
id: story:adapter-io-spec
kind: story
status: draft
title: The confinement specification states adapter standard input
relations:
- decomposes: epic:confinement-contract-family
revision: 3
---
# The confinement contract states standard input

## Design

[Design 32](../../../docs/design/32-adapter-stdin-and-live-output.md) § 6 (stdin half), § 11
story 1, D7 and D9: the spec change for `stdin_pipe` only, made by a spec story because ADR 0044
decision 6 keeps adapter stories out of the suite. Live output has its own spec story,
`story:adapter-live-spec`. No adapter code and no adapter proof.

## Depends on

None.

## Acceptance

- `spec/confinement/confinement.yaml`: `Fact` gains `stdin_pipe` after `image_digest_pinned`;
  `AdmitProfile` gains `fact-unserved-stdin-pipe`; the declared sets above `OpenAdapter` state
  § 5's `stdin_pipe` with its source, the `oci` set for the oci lane's pinned runtime, the `pod`
  set unchanged.
- Every open step that states the declared set of `local`, `linux`, `systemd`, `macos` or `oci`
  names `stdin_pipe`, and the eight stdin scenarios of § 6's table exist.
- `cargo xtask check-ess` regenerates `spec/confinement-suite.json` and `Fact`; `FACT_ORDER` is
  `[Fact; 12]` and the crate compiles; the public-items and `check-docs` fact-table lines move.
- The pinned counts move as § 6's table states: `PORTABLE_DECLARED` 19, `PORTABLE_EXECUTED` 18/19,
  `LINUX_DELEGATED` 10, `MACOS_DECLARED` 9 with a new `MACOS_IO` 1 admitted by the macos stage
  assertion; the README states portable 19, delegated 17, macos 9, oci 6, pod 4, interpreted 2
  and the obligation counts `check-ess` reports.
- It lands with `story:adapter-stdin-pipe` as one unit, merged only with every lane green on the
  unit's head. `pod-stdin-pipe-unserved-on-pod` passes from this story's commit on.
