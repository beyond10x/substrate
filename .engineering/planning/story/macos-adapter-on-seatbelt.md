---
format: aep.planning-md/3
id: story:macos-adapter-on-seatbelt
kind: story
status: draft
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
  path: crates/substrate-host/src/confinement/mod.rs
- confidence: inferred
  path: crates/substrate-host/src/seatbelt.rs
- confidence: inferred
  path: crates/substrate-host/tests/ess_confinement_macos.rs
revision: 2
---
# S1: the `macos` adapter on the Seatbelt primitive

ADR 0037 step 2 and ADR 0109 S1. The `macos` confinement adapter runs a command under
`SeatbeltPolicy` (`crates/substrate-host/src/seatbelt.rs`), opened through
`confinement::open`. Its facts: `process_run`, `process_watch`, `output_read`, `wait`, `workspace`,
`filesystem_view` and `audit` served; `limits`, `network_egress` and `network_ingress` not served
(ADR 0037 "What Seatbelt cannot confine": no pids or memory bound, no whole-tree kill, no
forwarder); `paths_are_host_paths: true`, so a read-only root whose mount differs from its host path
is refused by name. It fails closed on `seatbelt::probe()` with `confinement.adapter-unserved`.

The adapter is substrate-host API only: no wire change, and the daemon's `HostDriver::open` keeps
refusing `host.platform-unserved` on macOS until the successor-bundle story outside this epic.

## Acceptance

- `crates/substrate-host/tests/ess_confinement_macos.rs` (`#![cfg(target_os = "macos")]`) executes
  every `macos` scenario of `spec/confinement-suite.json` against the real adapter on the `macos-15`
  job of `.github/workflows/macos.yml`, which greps that test's own `test result: ok. <N> passed; 0
  failed` with `<N>` the pinned count; the job is green on the story's pull request.
- On that job, a profile needing `limits` is refused `confinement.family-unserved` naming `limits`,
  and one needing `network_egress` naming `network_egress`; nothing runs.
- The seven existing `seatbelt_adversary` cases still pass on the same job.
- On Linux the adapter is absent: `confinement::open(macos)` returns
  `confinement.adapter-unserved` (portable test).
- No claim of a local macOS run anywhere in the change; the only macOS evidence is the job's log.
- Adversary pass plus security-reviewer pass (confinement).

## Scope

`crates/substrate-host/src/confinement/macos.rs` (new), `crates/substrate-host/src/confinement/mod.rs`
(registration), `crates/substrate-host/src/seatbelt.rs` (only what the adapter needs exported),
`crates/substrate-host/tests/ess_confinement_macos.rs` (new), `.github/workflows/macos.yml`,
`CHANGELOG.md`.

## Depends on

`story:confinement-contract-on-linux-and-local`.
