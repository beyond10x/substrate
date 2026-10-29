---
format: aep.planning-md/3
id: review-result:design-34-u3-security-pass-1
kind: review-result
status: active
title: Security review of design 34 U3, the backend ledger, pass 1 (1 warning, 2 notes, 1 deferral)
relations:
- reviews: story:kubernetes-namespace-workspace-and-exec-slice
revision: 1
---
# Security review of design 34 U3 (backend ledger), pass 1

Reviewed `unit/backend-ledger` at `8a760c8a`, 2026-10-29. Verdict: needs change, one warning and
two notes introduced by the unit plus one deferral; all fixed in `58c3b8fd`, merged into
`wave/k8s-namespace-1` as `6adeb2d8`.

## Findings

| # | where | severity | finding | fix |
|---|---|---|---|---|
| F1 | `crates/substrate-daemon/src/app/service.rs:600` | warning | the dispatch-absence branch of design 34 § 5.1 read an absent backend object as terminal without the daemon checking the dispatch bound itself; a driver that answered absence early would settle an exec that could still appear | `Driver::dispatch_bound() -> Option<Duration>`, default `None`; the daemon compares the bound with its own clock and the row's `created_at`; before the bound the exec is skipped so the next recovery round retries it |
| F2 | `crates/substrate-store/src/lib.rs` (`SCHEMA_VERSION` 1 to 2) | note | the upgrade is one-way: 0.15.x and earlier refuse the upgraded store with `NewerSchema`, and the CHANGELOG did not say so | schema stays 2; CHANGELOG `[Unreleased]` states the one-way upgrade and to back up the state directory before upgrading |
| F3 | `crates/substrate-store/src/backend.rs` (`unsettled`) | note | `unsettled()` returned at most 256 rows with no cursor; rows past 256 were never reconciled | `unsettled(after)` pages by `(created_at, name)`; a test inserts 300 rows and reaches all of them |
| F4 | `crates/substrate-daemon/src/app/service.rs:540` | deferral | a workspace `NotFound` skips the ledger | comment only; U8 adds the workspace ledger row and its bound |

## After the fix (`58c3b8fd`), each run once

Store crate 120 passed (119 before); daemon `--test backend_ledger` 5 passed (4 before),
`--test driver_port` 3 passed; `cargo fmt --all --check` clean; clippy on store, host and daemon
with `--all-targets -D warnings` clean. Each new case failed before its fix (the cursor did not
advance; two daemon cases ended `Terminal` where `Unknown` is required).

## Not verified

No real driver states a dispatch bound yet, so the bounded branch runs only against the test
double. Full gate and delegated lane not run for this unit; they run once at the end of the wave.
