---
format: aep.planning-md/1
id: review-result:adversary-u3-pass-2
kind: review-result
status: active
title: Adversary pass 2, unit u3 (unattached session containment)
relations:
- reviews: story:unattached-claimed-session-is-contained
revision: 1
---
# Adversary pass 2 — unit u3, `story:unattached-claimed-session-is-contained`

Agent type `adp:adversary`. Worktree `wt-b180f053180d`, branch `impl/unattached-session-contained`,
HEAD `f0c387e`, base `617bbed`. Report as returned.

```
unit: u3 story:unattached-claimed-session-is-contained
verdict: red
cases: executed 132→133, red 1
origin: introduced 2 / pre-existing 2 / undecided 0
wrote-outside-worktree: 4 paths
needs-coordinator: no
```

## Case added

`crates/substrate-daemon/tests/pipe_session.rs:2716-2804`, +89, one path, no implementation file
touched. `adversary_a_served_attachment_with_an_unproven_kill_keeps_the_bounded_capacity` — the
round-1 capacity case **with the upgrade allowed to succeed**, so `on_upgrade` runs.

```
thread 'adversary_a_served_attachment_with_an_unproven_kill_keeps_the_bounded_capacity' panicked at
crates/substrate-daemon/tests/pipe_session.rs:2787:13:
attachment 31 of the bounded 32 is still answered 429 ten seconds after one served attachment whose
containment kill could not be proven: sessions.rs:1338 retained the tombstone and
PipeAttachmentPermit::drop then ran global.forget(), so one of this daemon's 32 attachment slots is
gone until restart. The durable claim already barred that session, so the tombstone bought nothing
-- which is the rule this unit's own sessions.rs:131-135 states.
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 28 filtered out; finished in 11.32s
```

Suite: exit 101, 133 executed across 16 targets, `pipe_session` 28 passed / 1 failed, every other
target green including `runtime_vectors` (the known flake did not fire).

## Findings

**1 — `sessions.rs:1338`, blocker, `pre-existing`.** The slot leak round 1 closed on the
failed-upgrade path is still open on the **ordinary served-attachment path**.
`terminate_pipe_session` (`:1871-1887`) returns false whenever `driver.signal` errors, exceeds
`MAINTENANCE_DRIVER_TIMEOUT`, or the observation write fails — no flag, no fixture-only state; it is
what a wedged driver does on any client disconnect. `Drop` then runs `global.forget()` (`:150`),
removing a permit from the fixed `Semaphore` for the process's life. **32 such events and the daemon
serves zero attachments until restart**, answering `session.attachment-capacity` — a row published
as *exhausted* and *retriable*, for a capacity that is destroyed and that no retry recovers.
That is `AGENTS.md` invariant 3.

**2 — `sessions.rs:131`, blocker, `introduced`.** The doc comment **this diff added** states the
tombstone must go "never on a path the durable claim already bars, which would pay a permanent slot
for a bar that is already in place" — and `:1338`, the one call site it left standing, is exactly
such a path. The claim is `Attached` before `on_upgrade` runs and never returns to `Available`:
`claim_pipe_session_attachment` answers `AlreadyClaimed` for `attached|consumed|uncertain`
(`substrate-store/src/sessions.rs:735-739`), the handler gate refuses anything but `Available`
before `acquire` (`sessions.rs:1150-1153`), and only `finish_pipe_session_start` sets `Available`
(`app/operations.rs:1053`). **The unit ships a rule and a violation of it in one commit, green at
every gate step.**

**3 — `tests/pipe_session.rs:2428`, warning, `introduced`.** The doc of
`adversary_a_stranded_attach_with_an_unproven_kill_keeps_the_bounded_capacity` still says the
`on_failed_upgrade` closure "calls `retain_attachment_tombstone()` whenever `terminate_pipe_session`
returns `false`". The correction removed that call; the closure's own comment now reads "No tombstone
here, deliberately". The case is green **because** the described call is gone, and its doc tells the
next reader it is present.

**4 — `tests/pipe_session.rs:2661`, note, `INFEASIBLE`.** The RST case's `!claimed.is_empty()` guard
is satisfied by any round, including one caught by the pre-existing `on_upgrade`/EOF hand-off, and
nothing observes *which* hand-off ran; its filter GET also races the handler's claim. **Nothing
found:** 24 rounds green across 16 isolated runs, 8 parallel, 5 pinned to one CPU and 3 to two —
32/32 passes, no skip observed. The implementor's own `red-f3-base.log` shows the case red with
`app/sessions.rs` reverted, so it does gate the containment as a whole.

**5 — `sessions.rs:91`, note, `pre-existing`.** `PipeAttachmentPermit.exec_id` and
`PipeAttachmentLimits::acquire(scope, exec_id)` are passed a **session** id (`:1170` passes
`&session_id`), while `terminate_pipe_session(app, scope, exec_id)` in the same file takes a real
exec id. Nothing breaks — both the insert and the `Drop` remove use the same key.

**Origin method.** `pre-existing` on 1 and 5 was established with
`git show 617bbed:crates/substrate-daemon/src/app/sessions.rs`: `acquire`, `Drop`,
`retain_attachment_tombstone`'s body and the entire `on_upgrade` closure are byte-identical to HEAD,
and the diff's only two hunks are the doc comment and the `on_failed_upgrade` block — neither of
which the new case executes. The base checkout was not moved; it is another agent's tree.

**Named fix for 1 and 2, not applied.** Delete `retain_attachment_tombstone`, `remove_key_on_drop`
and the `else` arm of `Drop`, and reduce `sessions.rs:1337-1339` to
`if !completed { let _ = terminate_pipe_session(&app, &scope, &exec_id).await; }` — the warn-log the
`on_failed_upgrade` branch already has belongs on this branch too. That restores the invariant the
diff's own new doc states and costs nothing, because the durable claim is the bar.

## Attacked and could not break

- **The F3 measurement's soundness** — the mechanism is library-level, not harness-level:
  `vendor/axum-0.8.9/src/extract/ws.rs:359-366` calls `on_failed_upgrade` whenever
  `hyper::upgrade::on(...)` resolves `Err`, which is what a dropped or errored connection future
  with a pending upgrade produces, **independent of `with_upgrades()`**. All three production
  listeners call `with_upgrades()` and additionally wrap the connection in
  `enforce_connection_lifetime` (`runtime.rs:200-210`), which *drops* the future at 5 minutes — a
  second, non-client trigger. Production reaches the closure at least as readily as the harness. Not
  observed on a real listener; sound in mechanism, not measured.
- **RST case stability** — 32 consecutive passes: 8 isolated (0.73–1.03 s), 8 parallel on a 20-core
  box (1.53–1.55 s), 5 pinned to `cpu 0`, 3 pinned to `cpu 0,1`. No flake.
- **The F1 fix's load-bearing claim** — that no route reaches `acquire` for a session whose claim was
  consumed. Every write to `SessionAttachmentState` enumerated; `Available` is set only at session
  start. Even a request whose gate read a stale `Available` is refused at the store. Holds, including
  across a daemon restart.
- **Caller enumeration for `retain_attachment_tombstone`** — complete; `grep` finds exactly one call
  site. The enumeration was right; the decision to keep it was not (finding 1).
- **F2's pinned case** — `sweep_expired` is the production tick (250 ms, `runtime.rs:534`) and the
  case drives it explicitly 20 times, so a backstop landing in `sweep_expired`/`cleanup_expired`
  trips it. It pins what it says.
- **`warn!` on the unproven branch fires**, reached via `spawn_without_upgrades` + `refuse_signal`.
- **`acquire`'s early return on `AlreadyAttached`** does not leak the global permit.

```findings
- file: crates/substrate-daemon/src/app/sessions.rs
  line: 1338
  category: mutant
  severity: blocker
  verdict: needs-revision
  origin: pre-existing
  message: >-
    on the ordinary served-attachment path one client disconnect whose kill the driver cannot prove
    permanently forgets a global attachment permit, so the daemon serves 31 of its 32 slots until
    restart and reports the loss as a retriable "exhausted" capacity; measured red at
    crates/substrate-daemon/tests/pipe_session.rs:2787, gate exit 101.
- file: crates/substrate-daemon/src/app/sessions.rs
  line: 131
  category: contract-drift
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: >-
    the doc comment this diff adds forbids the tombstone "on a path the durable claim already bars",
    and the one call site the diff leaves standing at :1338 is exactly such a path, so the unit ships
    a rule and a violation of it in one commit, green at every step.
- file: crates/substrate-daemon/tests/pipe_session.rs
  line: 2428
  category: contract-drift
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: >-
    the doc of adversary_a_stranded_attach_with_an_unproven_kill_keeps_the_bounded_capacity still
    says the on_failed_upgrade closure calls retain_attachment_tombstone, which the F1 correction
    removed, so the case now passes because the behaviour it describes is gone.
- file: crates/substrate-daemon/tests/pipe_session.rs
  line: 2661
  category: property
  severity: note
  verdict: needs-revision
  origin: introduced
  message: >-
    the RST case's !claimed.is_empty() guard is satisfied by a round caught by the pre-existing
    on_upgrade EOF hand-off and nothing observes which hand-off ran, and its filter GET races the
    handler's claim, but no skip or flake could be exhibited in 32 runs across four scheduling
    regimes.
- file: crates/substrate-daemon/src/app/sessions.rs
  line: 91
  category: judgement
  severity: note
  verdict: needs-revision
  origin: pre-existing
  message: >-
    PipeAttachmentPermit.exec_id and PipeAttachmentLimits::acquire's exec_id parameter both hold a
    session id (:1170 passes &session_id) while terminate_pipe_session in the same file takes a real
    exec id, three lines from the machinery this unit changed.
```
