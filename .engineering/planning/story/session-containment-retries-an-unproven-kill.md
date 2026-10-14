---
format: aep.planning-md/3
id: story:session-containment-retries-an-unproven-kill
kind: story
status: active
title: Session containment retries an unproven kill
scope:
- confidence: cited
  path: crates/substrate-daemon/src/app/service.rs
- confidence: cited
  path: crates/substrate-daemon/src/app/sessions.rs
- confidence: cited
  path: crates/substrate-daemon/src/runtime.rs
- confidence: cited
  path: crates/substrate-daemon/tests/pipe_session.rs
revision: 10
transitions:
- {from: "draft", to: "proposed", at: "2026-10-14T17:24:21Z", actor: "human:timo", revision: 9, decided_on: {"recorded":{"review_outcome":6}}}
- {from: "proposed", to: "active", at: "2026-10-14T17:24:21Z", actor: "human:timo", revision: 10, decided_on: {"recorded":{"review_outcome":6}}}
---
# Story: Session containment retries an unproven kill

## Context

When a claimed pipe session must be contained — the client dropped, or the upgrade failed — the
daemon makes **one** attempt to end the process tree and does not try again.
`terminate_pipe_session` (`crates/substrate-daemon/src/app/sessions.rs:1925`) returns `false`
whenever `driver.signal` errors, exceeds `MAINTENANCE_DRIVER_TIMEOUT`, or the observation write
fails. It has two callers: the detached task in `on_failed_upgrade` (`sessions.rs:1341`) and the
`on_upgrade` future when an attachment ends without a terminal observation (`sessions.rs:1378`). On
the `false` branch of either, nothing retries, and no maintenance sweep picks the session up.

Measured during the 2026-09-04 security wave (`review-result:adversary-u3-pass-1`): with a 500 ms
transient driver refusal, a stranded exec is still `"running"` after 10 s and roughly 200 explicit
`App::sweep_expired()` ticks. The only backstop is `cleanup_expired`
(`crates/substrate-daemon/src/app/service.rs:643`), which acts on an **expired lease** — the very
"until its lease or timeout ended it" that `story:unattached-claimed-session-is-contained` calls the
defect it exists to remove.

The defect is pre-existing: it reproduces at `617bbed`, where the failed-upgrade path had no
containment at all.

## Decision (where the retry lives)

The retry runs in a **containment step of its own**, not inside `App::sweep_expired` and not under
its `lease_sweep` guard (`service.rs:323`), so a driver that hangs on a kill cannot stall
reconcile, lease cleanup or any other maintenance duty:

- An unproven containment from either caller records `(scope, exec_id)` in an in-memory set of
  pending containments the `App` owns, and wakes the containment step.
- `App::contain_pending()` (new, public like `sweep_expired`) attempts every pending entry
  **concurrently**, at most 64 per round, each bounded by `MAINTENANCE_DRIVER_TIMEOUT` (`app.rs:16`).
  An entry with an attempt in flight is not attempted twice.
- `runtime.rs` runs `contain_pending` on a 250 ms tick and on wake-up, in a task beside the lease
  sweeper (`runtime.rs:639`) that is aborted wherever the sweeper is (`:651`, `:661`, `:729`).
- **Drop rule.** An entry leaves the set when the kill is proven (`terminate_pipe_session` returns
  `true`), when the store records the exec terminal (`exited`, `cancelled`, `expired` or
  `unknown`), or when its lease has expired, at which point `cleanup_expired` (`service.rs:643`)
  owns it. A drop by lease expiry logs `tracing::warn!` naming the exec (invariant 3).
- The set is not durable. A daemon restart drops it, and `reconcile_after_restart`
  (`service.rs:327`) answers for an exec that was live across a restart, as it does today. This
  adds no wire field, no store column and no refusal.

Each failed attempt still logs `tracing::warn!`; the comments that say "nothing here retries it"
are corrected.

## Acceptance

A claimed session whose first containment attempt is refused by the driver leaves no running process
within one containment round once the driver recovers, for both callers of `terminate_pipe_session`,
proven by cases that refuse the first signal and allow the next and drive `App::contain_pending`;
and a case whose driver never answers the kill shows `App::sweep_expired` returning while that
attempt is still pending.

## Notes

`crates/substrate-daemon/tests/pipe_session.rs:2531` carries
`a_stranded_attach_whose_kill_is_unproven_is_only_ended_by_lease_expiry`, which pins today's
behaviour. **That case goes red when this story lands. Invert it, never relax it** — its own doc says
so and names this story.

`sweep_expired` is the production maintenance tick (250 ms, `crates/substrate-daemon/src/runtime.rs:640`);
the containment step gets its own tick of the same period.

## Parallel work

Shares `crates/substrate-daemon/src/app/sessions.rs` with
story:the-daemons-detached-containment-task-is-tracked, which `depends_on` this story. The two are
one wave unit worked by one implementor in sequence, this story first. No other unit of this wave
touches `sessions.rs`, `service.rs`, `runtime.rs` or `tests/pipe_session.rs`. The `CHANGELOG.md`
entry is written by the integrator at integration, not by the unit.
