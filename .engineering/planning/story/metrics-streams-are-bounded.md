---
format: aep.planning-md/1
id: story:metrics-streams-are-bounded
kind: story
status: implemented
title: Metrics WebSocket streams carry a permit, a lifetime and frame bounds
summary: GET /v1/metrics/stream upgrades with no cap, no lifetime and default frame bounds; one SQLite write per stream per second (metrics.rs:75-98).
tags:
- review
relations:
- decomposes: epic:review-2026-09-03-findings
scope:
- confidence: cited
  path: crates/substrate-daemon/src/app/events.rs
- confidence: cited
  path: crates/substrate-daemon/src/app/metrics.rs
- confidence: cited
  path: crates/substrate-daemon/src/app/service.rs
- confidence: cited
  path: crates/substrate-daemon/src/app/tests.rs
- confidence: cited
  path: crates/substrate-daemon/tests/metrics_stream_adversary.rs
- confidence: cited
  path: website/docs/guides/storage-and-metrics.md
revision: 13
---
# Story: Metrics streams are bounded

## Context

`GET /v1/metrics/stream` upgrades to a WebSocket with no permit, no lifetime and the library's
default frame and message bounds (`crates/substrate-daemon/src/app/metrics.rs:75-90`). Each open
stream runs `load_exec_usage` once a second, which is an `observe_exec` plus a `put_exec` SQLite
write (`metrics.rs:98-` and `:215-`), until the exec ends, up to the 24 h exec timeout. Event
streams are capped at 64 global and 4 per subject (`app/events.rs:47-48`) and pipe attachments at
32 (`app/sessions.rs:63`); metrics streams at nothing. One authenticated uid can open as many as
the kernel allows.

## Acceptance

Opening one more metrics stream than a published per-subject cap answers `429` with a named `exhausted` refusal.

## Notes

Reuse the `EventStreamLimits` shape and `EventStreamPolicy` bounds; the metrics stream is the one upgrade that has none of them. The lifetime and the client-message bound arrive with the same policy struct and are checked by their own cases, not by the acceptance above.

## Parallel work

This story shares `crates/substrate-daemon/src/app/service.rs` with
story:lease-cleanup-reads-exec-state-only; the two touch different functions (stream limits on
`App` versus `cleanup_expired`) but land on one file, so they are worked in sequence, not at once.

**What the 2026-09-04 wave learned, which no scope entry expressed.** This story's own class check
reads every `.rs` file under `crates/substrate-daemon/src/app/`, so it is coupled to
`app/sessions.rs` and `app/events.rs` even though it edits neither. Working this story beside one
that restructures an upgrade builder chain in those files turns the package gate green on both
branches and red on the merge. That happened: story:unattached-claimed-session-is-contained
inserted a closure between `sessions.rs`'s frame bounds and its `.on_upgrade(`, and the check read
an empty chain. The check now skips balanced brackets, but the coupling is a property of a check
that reads its siblings, not of that one defect.
