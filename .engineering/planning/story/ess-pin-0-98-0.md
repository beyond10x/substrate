---
format: aep.planning-md/3
id: story:ess-pin-0-98-0
kind: story
status: archived
title: The ESS pin moves to 0.98.0
tags:
- maintenance
scope:
- confidence: cited
  path: .github/workflows/gate.yml
- confidence: cited
  path: STATUS.md
- confidence: cited
  path: crates/substrate-host/src/generated/confinement.rs
- confidence: cited
  path: crates/substrate-wire/src/generated
- confidence: cited
  path: spec
revision: 4
transitions:
- {from: "draft", to: "archived", at: "2026-11-25T19:09:43Z", actor: "agent:substrate", revision: 4}
---
# Story: the ESS pin moves to 0.98.0

## Outcome

The six `ess-inputs.yaml` under `spec/`, and `ESS_VERSION` and `ESS_SHA256` in `gate.yml`, name
ESS 0.98.0, and the committed projections are what that release regenerates.

## What is known

2026-11-23, with the 0.98.0 release binary on a copy of `spec/` pinned to 0.98.0: all six
specifications validate with `--strict-requires` (21, 5 and 74 scenarios in the three that have
them). The archive `ess-0.98.0-x86_64-unknown-linux-gnu.tar.gz` has the SHA-256
`c6c9a3a3d6956ce1a237f0340905dfadc431d31b1a15b104a6d2e64bb5acaa8e` (`sha256sum` on the download).
Synthesis and the regenerated projections were not run.

## What changes

The pin and the digest move in one commit with the regenerated suites and generated types.
Synthesis of the confinement suite is timed before and after (0.97.4: 28 s). The local gate uses
the installed `ess`, so the change waits until the host has 0.98.0 installed.

## Acceptance

`cargo xtask check-ess` exits 0 locally and in the forge gate, and the change names the scenarios
whose bytes moved.
