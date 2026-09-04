---
format: aep.planning-md/1
id: story:metrics-streams-are-bounded
kind: story
status: active
title: Metrics WebSocket streams carry a permit, a lifetime and frame bounds
summary: GET /v1/metrics/stream upgrades with no cap, no lifetime and default frame bounds; one SQLite write per stream per second (metrics.rs:75-98).
tags:
- review
relations:
- decomposes: epic:review-2026-09-03-findings
scope:
- confidence: cited
  path: crates/substrate-daemon/src/app/metrics.rs
- confidence: cited
  path: crates/substrate-daemon/src/app/service.rs
revision: 7
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

This story shares `crates/substrate-daemon/src/app/service.rs` with story:metrics-streams-are-bounded and story:lease-cleanup-reads-exec-state-only; the two touch different functions (stream limits on `App` versus `cleanup_expired`) but land on one file, so they are worked in sequence, not at once.
