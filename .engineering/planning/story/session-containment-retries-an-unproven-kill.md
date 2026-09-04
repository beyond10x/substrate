---
format: aep.planning-md/1
id: story:session-containment-retries-an-unproven-kill
kind: story
status: draft
title: Session containment retries an unproven kill
scope:
- confidence: cited
  path: crates/substrate-daemon/src/app/service.rs
- confidence: cited
  path: crates/substrate-daemon/src/app/sessions.rs
- confidence: cited
  path: crates/substrate-daemon/tests/pipe_session.rs
revision: 4
---
# Story: Session containment retries an unproven kill

## Context

When a claimed pipe session must be contained — the client dropped, or the upgrade failed — the
daemon makes **one** attempt to end the process tree and does not try again.
`terminate_pipe_session` (`crates/substrate-daemon/src/app/sessions.rs`) returns `false` whenever
`driver.signal` errors, exceeds `MAINTENANCE_DRIVER_TIMEOUT`, or the observation write fails. On
that branch nothing retries, and no maintenance sweep picks the session up.

Measured during the 2026-09-04 security wave (`review-result:adversary-u3-pass-1`): with a 500 ms
transient driver refusal, a stranded exec is still `"running"` after 10 s and roughly 200 explicit
`App::sweep_expired()` ticks. The only backstop is `cleanup_expired`
(`crates/substrate-daemon/src/app/service.rs:645`), which acts on an **expired lease** — the very
"until its lease or timeout ended it" that `story:unattached-claimed-session-is-contained` calls the
defect it exists to remove.

The defect is pre-existing: it reproduces at `617bbed`, where the failed-upgrade path had no
containment at all. `story:unattached-claimed-session-is-contained` made the situation strictly
better and deliberately did not close this half.

## Acceptance

A claimed session whose first containment attempt is refused by the driver leaves no running process
within one maintenance tick once the driver recovers, proven by a case that refuses the first signal
and then allows the second.

## Notes

`crates/substrate-daemon/tests/pipe_session.rs` already carries
`a_stranded_attach_whose_kill_is_unproven_is_only_ended_by_lease_expiry`, which pins today's
behaviour: the exec is still `running` across 20 driven maintenance ticks, and renewing to
`MIN_LEASE_TTL_MS` then expiring yields `state == "expired"`. **That case goes red when this story
lands. Invert it, never relax it** — its own doc says so and names this story.

`sweep_expired` is the production tick (250 ms, `crates/substrate-daemon/src/runtime.rs:534`), so a
backstop placed in `sweep_expired`/`cleanup_expired` trips that case. A backstop on its own timer
slower than the ~500 ms window would leave it green; the adversary judged that unlikely and did not
file it separately.
