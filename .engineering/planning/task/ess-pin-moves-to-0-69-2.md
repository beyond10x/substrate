---
format: aep.planning-md/3
id: task:ess-pin-moves-to-0-69-2
kind: task
status: active
title: Move the ESS pin to 0.69.2
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-26T14:02:17Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-26T14:02:18Z", actor: "human:timo", revision: 4}
---
# Task: move the ESS pin to 0.69.2

## Context

The workspace rule is to use the newest ESS release everywhere (operator, 2026-10-05: "ALWAYS
upgrade to most recent ESS"). Session b10x-chef-orchestrator approved this move on 2026-10-26 as a
class-C dependency upgrade. The previous move, to 0.68.0, is `task:ess-pin-moves-to-0-68-0`
(commit `36f90c3`, digest fix `711098d`).

ESS 0.69 names a generated closed-vocabulary variant after its spelling (`Recorded`, `Adopted`,
`ExpiresAt`) where 0.68.0 emitted `V0`, `V1`, …; the hand-written associated constants in
`crates/substrate-wire/src/lib.rs` that gave those variants their names collide with them and go.
0.69.2 also synthesizes `StartProcess/name-conflict` and both stop-by-owner branches, which 0.68.0
refused, so the host-process suite moves from 88 synthesized and 31 refusals to 91 and 28.

## Acceptance

Every `ess-inputs.yaml` requires ess 0.69.2. `.github/workflows/gate.yml` installs 0.69.2 with its
release digest and `xtask/src/ess.rs` names it. The suites, generated wire types and CLI package
are regenerated with 0.69.2; `spec/coverage.md` carries the 0.69.2 synthesis counts. `cargo xtask
check-ess` exits 0, and so does `bash scripts/gate.sh`.
