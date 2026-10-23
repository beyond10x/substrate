---
format: aep.planning-md/3
id: task:ess-pin-moves-to-0-68-0
kind: task
status: implemented
title: Move the ESS pin to 0.68.0
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-23T13:44:22Z", actor: "agent:b10x-chef-orchestrator", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-23T13:44:22Z", actor: "agent:b10x-chef-orchestrator", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-23T14:58:47Z", actor: "agent:b10x-chef-orchestrator", revision: 4, decided_on: {"recorded":{"verification":2}}}
---
# Task: move the ESS pin to 0.68.0

## Context

The wave 2026-10-23 integration gate stopped at its first step, `cargo xtask check-ess`:
`spec/ess-inputs.yaml requires ess 0.67.0 and this is ess 0.68.0, which is newer, and
--strict-requires refuses a newer release`. The installed ESS is 0.68.0, and the workspace rule is
to use the newest ESS release everywhere. The previous move, to 0.67.0, is commit `ea150b2`.

## Acceptance

Every `ess-inputs.yaml` requires ess 0.68.0. `.github/workflows/gate.yml` installs 0.68.0 and
`xtask/src/ess.rs` names it. The suites and generated wire types are regenerated with 0.68.0, and
the spec READMEs and `spec/coverage.md` carry the 0.68.0 synthesis counts. `cargo xtask check-ess`
exits 0, and so does the full gate.
