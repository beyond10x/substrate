---
format: aep.planning-md/3
id: story:macos-adapter-on-seatbelt
kind: story
status: implemented
title: 'S1: the macos adapter on the Seatbelt primitive'
owner: substrate
tags:
- adr-0109
relations:
- decomposes: epic:confinement-contract-family
- depends_on: story:confinement-contract-on-linux-and-local
- informed_by: story:macos-seatbelt-confinement
scope:
- confidence: inferred
  path: .github/workflows/macos.yml
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: crates/substrate-host/src/confinement/macos.rs
- confidence: inferred
  path: crates/substrate-host/src/seatbelt.rs
- confidence: inferred
  path: crates/substrate-host/tests/ess_confinement_macos.rs
revision: 8
transitions:
- {from: "draft", to: "proposed", at: "2026-10-27T01:13:03Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"review_outcome":1}}}
- {from: "proposed", to: "active", at: "2026-10-27T01:13:04Z", actor: "human:timo", revision: 7, decided_on: {"recorded":{"review_outcome":1}}}
- {from: "active", to: "implemented", at: "2026-10-27T07:47:38Z", actor: "human:timo", revision: 8, decided_on: {"recorded":{"test_result":3,"review_outcome":3}}}
---
# S1: the `macos` adapter on the Seatbelt primitive

ADR 0037 step 2 and ADR 0109 S1. The `macos` confinement adapter runs a command under
`SeatbeltPolicy` (`crates/substrate-host/src/seatbelt.rs`), opened through
`confinement::open`. Its facts: `process_run`, `process_watch`, `output_read`, `wait`, `workspace`,
`filesystem_view` and `audit` served; `limits`, `network_egress` and `network_ingress` not served
(ADR 0037 "What Seatbelt cannot confine": no pids or memory bound, no whole-tree kill, no
forwarder); `paths_are_host_paths` proved, so a read-only root whose mount differs from its host
path is refused by name; `whole_tree_kill` not proved, refused `confinement.fact-unserved`. It
fails closed on `seatbelt::probe()` with `confinement.adapter-unserved`.

The adapter is substrate-host API only: no wire change, and the daemon's `HostDriver::open` keeps
refusing `host.platform-unserved` on macOS until the successor-bundle story outside this epic.
There is no macOS host here: the code is iterated and proved only on the `macos-15` job of
`.github/workflows/macos.yml`, run through `workflow_dispatch` on the unit branch (dispatched
through the App) and on the integration pull request.

## Acceptance

- `crates/substrate-host/tests/ess_confinement_macos.rs` (`#![cfg(target_os = "macos")]`) executes
  every `macos-s1-*` scenario and every other scenario tagged `macos` of
  `spec/confinement-suite.json` against the real adapter on the `macos-15` job, which greps that
  test's own `test result: ok. <N> passed; 0 failed` with `<N>` the pinned count.
- The same test asserts that `sealed_paths`, `exec_subpaths` and `private_tmpdir` are absent from
  the adapter's facts and that a profile requiring each is refused `confinement.fact-unserved`
  naming it; the assertion message names `story:macos-seatbelt-policy-runs-cargo-test`, which
  flips it.
- On that job, a profile needing `limits` is refused `confinement.family-unserved` naming `limits`,
  one needing `network_egress` naming `network_egress`, and one requiring `whole_tree_kill` is
  refused `confinement.fact-unserved` naming it; nothing runs.
- The seven existing `seatbelt_adversary` cases still pass on the same job.
- On Linux the adapter is absent: `confinement::open(macos)` returns
  `confinement.adapter-unserved` (portable test).
- The green `macos-15` run of the unit's final head is recorded as `test_result` evidence on this
  story with `--ref <run URL>`; that record is the story's only macOS evidence.
- An `aep:adversary` pass and an `aep:security-reviewer` pass are each recorded as a
  `review-result` that `reviews:` this story, and each of their findings has a `review_outcome`
  record: `fixed` in this story, or `escalated` with a filed story id.

## Scope

`crates/substrate-host/src/confinement/macos.rs` (replaces the S0 stub),
`crates/substrate-host/src/seatbelt.rs` (only what the adapter needs exported),
`crates/substrate-host/tests/ess_confinement_macos.rs` (new), `.github/workflows/macos.yml`,
`CHANGELOG.md` (one line under `## [Unreleased]`; the other units of its wave add theirs, and the
merge into the integration branch keeps every line).

## Depends on

`story:confinement-contract-on-linux-and-local`.
