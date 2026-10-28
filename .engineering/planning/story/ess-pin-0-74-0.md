---
format: aep.planning-md/3
id: story:ess-pin-0-74-0
kind: story
status: implemented
title: The ESS pin moves to 0.74.0, wire bytes unchanged
scope:
- confidence: inferred
  path: .github/workflows/gate.yml
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: crates/substrate-host/src/generated
- confidence: inferred
  path: crates/substrate-wire/src/generated
- confidence: inferred
  path: crates/substrate-wire/src/lib.rs
- confidence: inferred
  path: spec
- confidence: inferred
  path: xtask/src/ess.rs
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-28T08:43:36Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-28T08:43:36Z", actor: "human:timo", revision: 4, executor: "agent:substrate-adr-0109"}
- {from: "active", to: "implemented", at: "2026-10-28T10:13:59Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"test_result":1}}}
---
# Story: the ESS pin moves to 0.74.0, wire bytes unchanged

## Outcome

Every specification under `spec/` and the CI gate's `ess` install run ESS 0.74.0, the newest
release (https://github.com/beyond10x/ess/releases/tag/0.74.0, tag at ess main `085f893e`).

## Why

Standing operator rule, 2026-10-05: always upgrade to the most recent ESS. The orchestrating
session requested the move on 2026-10-28 under its grant.

## Acceptance

- Every `requires: ess` line in `spec/**/ess-inputs.yaml` names `0.74.0`. The constants that
  name the release in `xtask/src/ess.rs` and `crates/substrate-wire/src/lib.rs` name it too, and
  so does every generated header.
- `ESS_VERSION` and `ESS_SHA256` in `.github/workflows/gate.yml` match the published Linux archive.
  The digest is checked with `sha256sum` on the downloaded file.
- `cargo xtask check-ess` exits 0.
- `git diff --stat -- contracts/` is empty, and `check-bundles` passes.
- `bash scripts/gate.sh` exits 0.
- The 0.74.0 library rename `SeedAdmissionError::CallerModel` → `CallerContradiction` has no use
  in this repository (`grep -rn 'CallerModel\|SeedAdmission' crates xtask spec`, empty on 2026-10-28).
- A failure caused by ESS itself is reported to the orchestrating session as an ESS gap, not
  worked around.
