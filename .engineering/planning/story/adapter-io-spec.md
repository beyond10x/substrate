---
format: aep.planning-md/3
id: story:adapter-io-spec
kind: story
status: draft
title: The confinement specification states adapter standard input and live output
relations:
- decomposes: epic:confinement-contract-family
revision: 1
---
# The confinement contract states standard input and live output

## Design

[Design 32](../../../docs/design/32-adapter-stdin-and-live-output.md) § 6, D7 and D9: every spec
change of the design, in one story that lands first (ADR 0044 decision 6). No adapter code.

## Acceptance

- `spec/confinement/confinement.yaml`: `Fact` gains `stdin_pipe` and `output_live` after
  `image_digest_pinned`; `AdmitProfile` gains `fact-unserved-stdin-pipe` and
  `fact-unserved-output-live`; the declared sets above `OpenAdapter` state § 5's facts with their
  sources, the `oci` set for the oci lane's pinned runtime and the `pod` set unchanged.
- Every authored scenario that opens `local`, `linux`, `systemd`, `macos` or `oci` states the new
  declared set, and the nine scenarios of § 6's table exist.
- `cargo xtask check-ess` regenerates `spec/confinement-suite.json` and `Fact` and passes against
  the `interpreted` reference; the README's lane counts and obligation counts match the suite.
- It lands first on the wave's integration branch. The portable, delegated, macos and oci lanes
  are red there until `story:adapter-stdin-pipe` and `story:adapter-live-output` land; the branch
  merges into `main` only with every lane green on its head. The two `pod` refusal scenarios pass
  from this commit on.
