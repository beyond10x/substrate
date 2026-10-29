---
format: aep.planning-md/3
id: story:ess-pin-0-76-0
kind: story
status: active
title: The ESS pin moves to 0.76.0, wire bytes unchanged
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-10-29T05:11:22Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-29T05:11:22Z", actor: "human:timo", revision: 3}
---
# Story: the ESS pin moves to 0.76.0, wire bytes unchanged

## Outcome

Every specification under `spec/` and the CI gate's `ess` install run ESS 0.76.0, the newest
release (https://github.com/beyond10x/ess/releases/tag/0.76.0).

## Why

Standing operator rule, 2026-10-05: always upgrade to the most recent ESS. The orchestrating
session requested the move on 2026-10-29, inside `wave/k8s-namespace-1` and not as its own pull
request.

## Acceptance

- Every `requires: ess` line in `spec/**/ess-inputs.yaml` names `0.76.0`. The constants that
  name the release in `xtask/src/ess.rs` and `crates/substrate-wire/src/lib.rs` name it too, and
  so does every generated header.
- `ESS_VERSION` and `ESS_SHA256` in `.github/workflows/gate.yml` match the published Linux archive.
  The digest is checked with `sha256sum` on the downloaded file.
- beyond10x/ess#685 publishes `minLength: 1` and `maxLength: 255` for the workspace root name;
  `crates/substrate-wire/src/generated/workspace.rs` is regenerated and differs only in its
  generator header line.
- `cargo xtask check-ess` exits 0.
- `git diff --stat -- contracts/` is empty.
- The wave's single local full gate (`bash scripts/gate.sh`) exits 0.
- A failure caused by ESS itself is reported to the orchestrating session as an ESS gap, not
  worked around.
