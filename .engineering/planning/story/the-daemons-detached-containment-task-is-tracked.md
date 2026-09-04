---
format: aep.planning-md/1
id: story:the-daemons-detached-containment-task-is-tracked
kind: story
status: draft
title: The daemon's detached containment task is tracked
scope:
- confidence: cited
  path: crates/substrate-daemon/src/app/sessions.rs
- confidence: cited
  path: crates/substrate-daemon/src/runtime.rs
revision: 3
---
# Story: The daemon's detached containment task is tracked

## Context

`crates/substrate-daemon/src/app/sessions.rs` contains the only detached `tokio::spawn` in the
daemon's non-test `app/` code. It runs the session containment that follows a failed WebSocket
upgrade. Verified by grep during the 2026-09-04 security wave: `tokio::spawn|spawn_blocking|JoinSet`
across non-test `src/app/` returns exactly that one site.

It is outside the connection `JoinSet` that `crates/substrate-daemon/src/runtime.rs:621` aborts at
shutdown, so a containment in flight when the daemon stops is neither awaited nor cancelled — it is
dropped with the runtime.

`story:unattached-claimed-session-is-contained` added the logging half of this
(`review-result:adversary-u3-pass-1`, finding F4) and could not add the tracking half: `runtime.rs`
belonged to another unit in that wave.

## Acceptance

A containment task still running when the daemon shuts down is either awaited or cancelled through
the same `JoinSet` that owns the connection tasks, proven by a case that starts one and shuts down
under it.

## Notes

The task already emits `tracing::info!` on containment and `tracing::warn!` on an unproven one, each
carrying `exec` and the `axum::Error`. What is missing is ownership, not observability.

Consider whether the task belongs in `runtime.rs:621`'s existing `JoinSet` or in a maintenance-owned
one; the first keeps one shutdown path, the second does not tie a session's containment to the
lifetime of the connection that triggered it.
