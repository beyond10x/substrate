---
format: aep.planning-md/3
id: review-result:wave-2026-10-14-u1-security-pass-1
kind: review-result
status: active
title: Security review pass 1, unit u1 (session containment)
relations:
- reviews: story:session-containment-retries-an-unproven-kill
- reviews: story:the-daemons-detached-containment-task-is-tracked
revision: 1
---
fixed

Security review pass 1 of the session-containment unit, read from its patches against a56705f9
(no build). No blocker.

Findings:
- **Warning, fixed.** Each containment round took the same first 64 pending entries in
  `HashMap` order, so while those stayed unproven any further entry was never attempted and ran
  until lease expiry. Now least-recently-attempted first; the reviewer's case
  `a_pending_containment_beyond_one_batch_is_reached_while_the_batch_stays_unproven` went red
  (0 kill attempts reached the driver for it) and is green (b004ed17).
- **Note, fixed.** An exec whose kill ended `unknown` (cgroup not proven empty) was dropped at
  `debug!`, and a kill answered `unknown` was logged as terminated. Both now warn; the drop to
  lease cleanup, which still treats `unknown` as live, is kept.
- **Note, fixed.** The containment tick bursts after a hung round, and every round warned per
  unproven entry. The tick now delays missed ticks; an unproven attempt warns on attempts 1, 2, 4,
  8 and so on. The tick setting has no test.
- **Note, fixed.** The no-detached-task guard matched only the literal `tokio::spawn` in
  top-level files before the first test module. It now scans `src/app.rs` and every file under
  `src/app/`, refuses any `spawn`, `spawn_*` or `JoinSet` token, and skips only test-module bodies.
- **Note, infeasible.** When the lease clock fails, an entry could be handed to a lease cleanup
  that the same failure skips; no reachable trigger was found.

Checked and sound: retried rather than forgotten on both callers; missing record dropped with a
warning; whole-tree kill unchanged; pending set bounded by the current-exec limit; containment
stopped on all three shutdown paths; no lock across an await; subject derivation untouched.
