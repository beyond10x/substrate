---
format: aep.planning-md/1
id: review-result:adversary-u3-pass-1
kind: review-result
status: active
title: Adversary pass 1, unit u3 (unattached session containment)
relations:
- reviews: story:unattached-claimed-session-is-contained
revision: 1
---
# Adversary pass 1 — unit u3, `story:unattached-claimed-session-is-contained`

Agent type `adp:adversary`. Worktree `wt-b180f053180d`, branch `impl/unattached-session-contained`,
HEAD `4cbc953` plus 3 uncommitted test cases, base `617bbed`. Report as returned.

```
unit: u3 story:unattached-claimed-session-is-contained
verdict: red
cases: executed 128→131, red 2
origin: introduced 3, pre-existing 2, undecided 0
wrote-outside-worktree: u3-target (assigned build dir, cargo artifacts only)
needs-coordinator: no
```

## Cases added

`crates/substrate-daemon/tests/pipe_session.rs`, +236, one path, no implementation file touched.
The fixture edit is additive — `PipeFixtureDriver.refuse_signal: AtomicBool`, default `false`,
guarded at the top of `signal`; all 25 pre-existing cases in that binary still pass.

| line | case | result |
|---|---|---|
| 2365 | `adversary_a_client_that_drops_after_the_switch_leaves_no_running_process` — the acceptance sentence on the *production* server shape (`with_upgrades()`), plus the refusal's class/status/retriable against the published `session.not-attachable` row | green |
| 2444 | `adversary_a_stranded_attach_with_an_unproven_kill_keeps_the_bounded_capacity` | **red** |
| 2523 | `adversary_a_stranded_attach_whose_kill_is_unproven_is_not_contained_by_maintenance` | **red** |

Red output, each run alone, before any suite run:

```
thread 'adversary_a_stranded_attach_with_an_unproven_kill_keeps_the_bounded_capacity' panicked at
crates/substrate-daemon/tests/pipe_session.rs:2488:13:
attachment 31 of the bounded 32 is still answered 429 twenty seconds after one stranded attach
whose kill could not be proven: the failed-upgrade containment retained the tombstone, and
PipeAttachmentPermit::drop then ran global.forget(), so one of this daemon's 32 attachment slots is
gone until restart. On 617bbed the same disconnect always returned the slot.
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 25 filtered out; finished in 21.26s
```

```
thread 'adversary_a_stranded_attach_whose_kill_is_unproven_is_not_contained_by_maintenance' panicked
at crates/substrate-daemon/tests/pipe_session.rs:2564:30:
a claimed session whose upgrade never completed is still running after ten seconds and many
maintenance ticks, because the single containment attempt was refused and nothing retries it; the
acceptance asks for no running process within one maintenance tick: Elapsed(())
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 26 filtered out; finished in 10.67s
```

Suite after the cases existed: `cargo test -p b10x-substrate-daemon --release --locked
--no-fail-fast`, exit **101**. 131 executed. `pipe_session` 25 passed / 2 failed; every other lane
green, including `runtime_vectors` 8/8 — the known flake did not fire.

## Findings

**F1 — one stranded attach whose kill is unproven permanently destroys a global attachment slot.**
`sessions.rs:1300`. Measured at `tests/pipe_session.rs:2488`, exit 101: after **one** stranded
attach with a refusing driver, only 31 of the daemon's 32 attachment slots are ever servable again;
the 32nd is `429 session.attachment-capacity` for the life of the process.
`retain_attachment_tombstone()` → `Drop` takes the `else` arm at `:151` and runs `global.forget()`.
Reachability: `on_failed_upgrade` is production-reachable — all three listeners call
`.with_upgrades()` (`runtime.rs:595`, `:918`, `:1083`), so a connection error before hyper fulfils
the upgrade resolves the future with `Canceled` and enters this closure. The
`terminate_pipe_session == false` half was **constructed** with a fixture driver that refuses
`signal`; the code's own doc comment and `MAINTENANCE_DRIVER_TIMEOUT` name that state as expected,
but no production driver was observed producing it. Origin `introduced`: at `617bbed` the file has
no `on_failed_upgrade`, so a failed upgrade always dropped the permit with
`remove_key_on_drop == true` and returned the slot. Fix not applied: do not retain the tombstone on
the failed-upgrade path — the durable claim is already `Consumed`, so the state gate at
`sessions.rs:1149` refuses every further attach *before* `acquire` at `:1167` is called. The
process-local tombstone cannot be consulted for that session again; it buys nothing and costs a slot.

**F2 — the containment is one attempt with no retry and no backstop.** `sessions.rs:1299`. Measured
at `tests/pipe_session.rs:2564`, exit 101: with a 500 ms transient driver refusal the stranded exec
is still `"running"` after 10 s and ~200 explicit `App::sweep_expired()` ticks. The acceptance asks
for "no running process within one maintenance tick". The only backstop is `cleanup_expired`
(`app/service.rs:645`), which acts on an **expired lease** — the very "until its lease or timeout
ended it" the story's Context calls the defect. Origin `pre-existing`: the residual reproduces at
`617bbed`, where this path has no containment at all, so HEAD is strictly better. It is an unclosed
part of the unit's own acceptance statement, not a regression.

**F3 — the unit's shipped case builds a state no daemon listener can be in.**
`tests/pipe_session.rs:989`. `spawn_without_upgrades` reaches `on_failed_upgrade` through hyper's
`Pending::manual()` — the branch taken only when `with_upgrades()` was **not** called. All three
daemon listeners call it. The adversary's green case at `:2365` shows a real client drop after the
`101` on a `with_upgrades()` listener is contained by the *other* hand-off (`on_upgrade` →
`run_pipe_attachment` sees EOF → `terminate_pipe_session`), on a code path byte-identical at
`617bbed`. Nothing production-side reaches `manual()`. The production trigger for the new closure is
`Canceled` after a connection error in the flush window, and no case in the tree builds it. So the
ordinary client drop the story describes was already contained on base, and the change's residual
value is unproven by its own suite. Origin `introduced` — it is about the case this unit added.

**F4 — the containment discards its own outcome and says nothing.** `sessions.rs:1298`. Read, not
run. The closure body is a bare `tokio::spawn`; no `warn!`, no event, no metric on either branch,
while `runtime.rs` and `service.rs` log every comparable maintenance failure. A containment that
fails is invisible — invariant 3's silent degradation. It is also the only detached `tokio::spawn`
in the daemon's non-test `app/` code, untracked by the connection `JoinSet` that `runtime.rs:621`
aborts at shutdown. Origin `introduced`; the sibling `on_upgrade` path at `:1319` is equally silent
and pre-existing.

**F5 — two doc comments about the same branch contradict each other, and F1 measures which is
false.** `sessions.rs:134` says "Capacity is still recovered"; `:149` says the branch "consumes one
of the fixed global attachment slots until daemon restart". F1's red case shows `:149` is the true
one. Origin `pre-existing` text; the unit edited exactly those lines (`&mut self` → `&self`) and
left the false sentence standing.

## Attacked, could not break

- **Memory ordering.** `Relaxed` store in `retain_attachment_tombstone` vs `get_mut()` in `Drop`:
  `Arc`'s `Release` decrement plus `Acquire` fence orders the store before the drop on any thread.
  Sufficient.
- **"Exactly one hand-off runs."** axum 0.8.9 `on_upgrade` (`ws.rs:359`) calls
  `on_failed_upgrade.call()` only in the `Err` arm and `callback` only in the `Ok` arm of one
  spawned task, and owns both closures — neither twice, never both, and the permit's `Drop` always
  runs after whichever ran.
- **"No fallible code between the claim and `.on_upgrade`."** Holds. `store_io` →
  `bounded_blocking` → `Semaphore::acquire_owned().await` then `block_in_place`; the claim and the
  whole tail of the handler complete inside one poll, so the handler future cannot be dropped
  between them.
- **Builder ordering.** `WebSocketUpgrade::on_failed_upgrade` copies every `WebSocketConfig` field,
  so placing it after the five buffer/frame builders loses none of them.
- **Refusal register conformance.** The contained session's refusal matches
  `contracts/substrate-wire/0.10.0/refusals.json`'s `session.not-attachable` row on class, status
  and retriable (asserted green at `:2365`).
- **The happy containment path.** When the kill succeeds, the slot and the `attached` key both come
  back — indices 0..30 of the capacity case take them.

```findings
- file: crates/substrate-daemon/src/app/sessions.rs
  line: 1300
  category: boundary
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: >-
    The failed-upgrade containment retains the attachment tombstone when it cannot prove the kill,
    and Drop then runs global.forget(), permanently destroying one of the 32 global attachment slots
    on a path that always returned the slot at 617bbed. One attach whose upgrade fails while the
    driver refuses the signal -> terminate_pipe_session returns false -> retain_attachment_tombstone
    -> Drop takes the else arm at sessions.rs:151 -> the daemon serves only 31 concurrent
    attachments for the rest of the process's life and answers the 32nd 429
    session.attachment-capacity forever. Measured at tests/pipe_session.rs:2488, exit 101.
- file: crates/substrate-daemon/src/app/sessions.rs
  line: 1299
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: pre-existing
  message: >-
    The containment is a single unretried kill with no maintenance backstop, so a transient driver
    refusal returns the daemon to the pre-fix behaviour the story exists to remove and the
    acceptance sentence "no running process within one maintenance tick" is not met. Stranded attach
    plus a 500 ms driver refusal, driver healthy again immediately -> the exec is still "running"
    after 10 s and ~200 explicit App::sweep_expired() ticks; the only backstop is cleanup_expired at
    app/service.rs:645, which fires on lease expiry, which is exactly what the story's Context calls
    the defect. Measured at tests/pipe_session.rs:2564, exit 101. Reproduces at 617bbed, so HEAD is
    strictly better and this is an unclosed part of the acceptance rather than a regression.
- file: crates/substrate-daemon/tests/pipe_session.rs
  line: 989
  category: judgement
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: >-
    The case shipped as proof reaches on_failed_upgrade through hyper's Pending::manual(), a branch
    taken only when with_upgrades() was not called, while all three daemon listeners call it, so the
    new code's production trigger is exercised by nothing in the tree. A real client dropping
    straight after the 101 on a with_upgrades() listener is contained by the other hand-off
    (on_upgrade -> run_pipe_attachment sees EOF -> terminate_pipe_session), a path byte-identical at
    617bbed; the adversary's green case at tests/pipe_session.rs:2365 shows it, so the story's
    stated scenario was already contained on base and the residual window the change covers is
    untested. Read plus one green case, not a red one.
- file: crates/substrate-daemon/src/app/sessions.rs
  line: 1298
  category: judgement
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: >-
    The containment runs in a bare detached tokio::spawn that discards its own outcome and logs
    nothing on either branch, so a containment that fails is invisible - invariant 3's silent
    degradation - and it is the only untracked task in the daemon's non-test app/ code, outside the
    connection JoinSet that runtime.rs:621 aborts at shutdown. terminate_pipe_session returns false
    -> no warn!, no event, no metric; the operator sees a session whose process is still running and
    no record that the daemon tried and failed to end it, while runtime.rs and app/service.rs log
    every comparable maintenance failure. Read, not run.
- file: crates/substrate-daemon/src/app/sessions.rs
  line: 134
  category: contract-drift
  severity: note
  verdict: needs-revision
  origin: pre-existing
  message: >-
    retain_attachment_tombstone's doc comment says "Capacity is still recovered" while Drop's own
    comment fifteen lines below says the branch "consumes one of the fixed global attachment slots
    until daemon restart"; the unit edited these exact lines and left the false sentence standing. A
    reader choosing where to call retain_attachment_tombstone is told capacity is safe;
    tests/pipe_session.rs:2488 measures that global.forget() runs and the slot never returns, so
    :149 is the true sentence and :134 is not.
```

## Coordinator note on this record

This agent was dispatched before the brief template's findings schema was corrected, so it returned
the block keyed `summary` / `failure_scenario` with severities `major` / `minor`. The block above is
the same five findings with those two fields merged into `message`, and severity mapped to the
store's vocabulary: `major` to `blocker` (F1, F2), `minor` to `warning` (F3, F4) and to `note` (F5,
a documentation contradiction). No finding was added, dropped or reworded beyond that merge. The
prose above the block is the agent's own text unedited.
