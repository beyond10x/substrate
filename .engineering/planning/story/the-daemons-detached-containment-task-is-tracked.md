---
format: aep.planning-md/3
id: story:the-daemons-detached-containment-task-is-tracked
kind: story
status: active
title: The daemon's detached containment task is tracked
relations:
- depends_on: story:session-containment-retries-an-unproven-kill
scope:
- confidence: cited
  path: crates/substrate-daemon/src/app/sessions.rs
- confidence: cited
  path: crates/substrate-daemon/src/runtime.rs
- confidence: cited
  path: crates/substrate-daemon/tests/pipe_session.rs
revision: 9
transitions:
- {from: "draft", to: "proposed", at: "2026-10-14T17:24:21Z", actor: "human:timo", revision: 8, decided_on: {"recorded":{"review_outcome":7}}}
- {from: "proposed", to: "active", at: "2026-10-14T17:24:21Z", actor: "human:timo", revision: 9, decided_on: {"recorded":{"review_outcome":7}}}
---
# Story: The daemon's detached containment task is tracked

## Context

`crates/substrate-daemon/src/app/sessions.rs:1341` is the only detached `tokio::spawn` in the
daemon's non-test `app/` code. It runs the session containment that follows a failed WebSocket
upgrade. Verified by grep during the 2026-09-04 security wave, and again on 2026-10-14
(`grep -rn tokio::spawn crates/substrate-daemon/src/app/`: `sessions.rs:1341`; the other hits are
test-only, in `app/tests.rs`, a `#[cfg(test)]` module (`app.rs:28`), and in `metrics.rs` inside its
`#[cfg(test)] mod tests` from line 415).

It is owned by nothing. The daemon's lease sweeper (`runtime.rs:639`) is aborted on shutdown on all
three transports (`runtime.rs:651`, `:661`, `:729`), and each transport aborts its own connection
`JoinSet` (`runtime.rs:673`, `:1015`, `:1099`); the containment task belongs to none of them, so one
in flight when the daemon stops is neither awaited nor cancelled — it is dropped with the runtime.

## Decision (one owner)

The owner is the **containment task** that story:session-containment-retries-an-unproven-kill adds
beside the lease sweeper, not a connection `JoinSet` and not `sweep_expired`. `on_failed_upgrade` no
longer spawns: it records the session as a pending containment and wakes that task, which makes the
first attempt at once (it does not wait behind a maintenance sweep, because the two are separate
tasks) and retries on its tick. That task is aborted wherever the lease sweeper is, on all three
transports (`runtime.rs:651`, `:661`, `:729`), so containment has one shutdown path and its lifetime
is not tied to the connection that triggered it. "Tracked" means **cancelled at shutdown**: an
attempt in flight when the daemon stops is aborted with the task, and the exec is then answered by
`reconcile_after_restart` on the next start, as any exec live across a restart is. No `JoinSet` is
added.

## Acceptance

`grep -rn tokio::spawn crates/substrate-daemon/src/app/` finds no hit outside `app/tests.rs` and
the `#[cfg(test)] mod tests` of `metrics.rs`; a failed upgrade's containment is performed by
`App::contain_pending`, proven by a case that fails an attachment's upgrade and observes the exec
contained after one driven round; and `runtime.rs` aborts the containment task on each of the three
shutdown paths that abort the lease sweeper.

## Notes

The containment already emits `tracing::info!` on success and `tracing::warn!` on an unproven one,
each carrying `exec` and the `axum::Error`; the move keeps both lines, carrying the error text into
the pending entry or logging it at enqueue.

## Parallel work

Shares `crates/substrate-daemon/src/app/sessions.rs` with
story:session-containment-retries-an-unproven-kill and `depends_on` it: the same implementor works it
second, in the same unit, because it reuses the pending-containment set and the containment task that story adds. No other
unit of this wave touches `sessions.rs`, `service.rs` or `runtime.rs`.
