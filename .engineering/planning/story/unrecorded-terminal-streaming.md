---
format: aep.planning-md/3
id: story:unrecorded-terminal-streaming
kind: story
status: draft
title: Stream interactive terminal output without durable capture
scope:
- confidence: cited
  path: .github/workflows/release.yml
- confidence: cited
  path: contracts/substrate-wire/0.17.0
- confidence: cited
  path: crates/b10x-substrate-sdk
- confidence: cited
  path: crates/substrate-daemon
- confidence: cited
  path: crates/substrate-host/src/process.rs
- confidence: cited
  path: crates/substrate-store
- confidence: cited
  path: crates/substrate-wire
- confidence: cited
  path: scripts/delegated-lane.sh
- confidence: cited
  path: scripts/gate.sh
- confidence: cited
  path: spec/sessions
- confidence: cited
  path: website/docs
- confidence: cited
  path: xtask
revision: 5
---
## Context

User requested implementation of GitHub issue #112. Source: crates/substrate-host/src/process.rs
`drain_capped` couples forwarding to retained bytes; crates/substrate-daemon/src/app/sessions.rs
`persist_pipe_observation` and app/service.rs maintenance persist observations. Store execs.rs
`upsert_exec` writes stdout/stderr BLOBs. Design: docs/design/23-terminal-capture-selection.md.
Typed selection: spec/sessions/capture.yaml (validated with pinned ESS).

## Acceptance

The named conformance scenarios in design 23 demonstrate policy-admitted PTY and pipe streaming
without durable payload capture on the production host/store paths, preserve finite independent
limits and metadata, and detect recording with a positive-control canary scan.

## Scope

Cited: crates/substrate-wire, crates/substrate-host/src/process.rs, crates/substrate-daemon/src,
crates/substrate-store/src/execs.rs, crates/b10x-substrate-sdk, xtask/bundle-source and contracts.
Inferred: successor 0.17.0 renderer/gate wiring, spec/sessions, public usage documentation.
All earlier released bundle bytes remain immutable. Mantle adoption is downstream and not acceptance.

## Implementation evidence

Implementation is available for review with native production tests, default-off deployment policy,
public SDK selection and successor development bundle 0.17.0. Evidence and remaining host-fixture
requirements are recorded in .engineering/reports/issue-112-unrecorded-streaming.md.
The lifecycle remains draft: this record does not claim design approval, merge, release,
or downstream Mantle acceptance. A live run with the complete resource-counter capability remains
required before acceptance; this workstation has no delegated io.stat.

## Release request

The operator requested a PR, merge and new Substrate release on 2026-10-02 after reviewing
implementation evidence and the exact synthetic-fixture admission change. Prepare version 0.7.9,
retain the documented measurement-fixture limitation, require the repository gates and record a
fresh delegated confinement run against the exact tagged main commit. Mantle adoption is separate.
