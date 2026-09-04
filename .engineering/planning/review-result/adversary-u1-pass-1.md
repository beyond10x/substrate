---
format: aep.planning-md/1
id: review-result:adversary-u1-pass-1
kind: review-result
status: active
title: Adversary pass 1, unit u1 (metrics stream caps)
relations:
- reviews: story:metrics-streams-are-bounded
revision: 1
---
# Adversary pass 1 — unit u1, `story:metrics-streams-are-bounded`

Agent type `adp:adversary`. Worktree `wt-d5a923d7143d`, branch `impl/metrics-streams-bounded`,
HEAD `377b115`, base `617bbed`. Report as returned.

```
unit: u1 — story:metrics-streams-are-bounded
verdict: red
cases: executed 130→135, red 2
origin: introduced 4 / pre-existing 1 / undecided 0
wrote-outside-worktree: 5 paths
needs-coordinator: no
```

## Cases added

`crates/substrate-daemon/tests/metrics_stream_adversary.rs`, 627 lines, untracked. No implementation
file touched.

| line | case | result |
|---|---|---|
| 453 | `a_metrics_stream_samples_at_the_interval_its_contract_advertises` | **red** |
| 602 | `the_metrics_sampling_cadence_does_not_depend_on_client_traffic` | **red** |
| 528 | `an_oversized_client_frame_ends_the_metrics_stream_and_returns_its_permit` | green |
| 547 | `the_metrics_stream_cap_is_not_shared_with_the_event_stream_cap` | green |
| 572 | `a_handshake_abandoned_before_its_first_sample_returns_its_permit` | green |

```
thread 'a_metrics_stream_samples_at_the_interval_its_contract_advertises' panicked at
crates/substrate-daemon/tests/metrics_stream_adversary.rs:411:5:
the metrics stream advertises a 1s sample interval and delivered its samples
[1.998987499s, 2.000531688s] apart (ceiling 1.5s)
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.16s

thread 'the_metrics_sampling_cadence_does_not_depend_on_client_traffic' panicked at
crates/substrate-daemon/tests/metrics_stream_adversary.rs:621:5:
a silent client is sampled every 2.000488208s and a client that sends one ping per sample every
999.768283ms (ratio 2.00); the cadence must not depend on what the client sends
```

Suite after the cases existed: exit 101, 135 executed, 133 passed, 2 failed. Every other lane green,
`runtime_vectors` 8/8.

## Findings

**F1 — the metrics stream samples at half its advertised rate.** `app/metrics.rs:176` with `:207`.
`run_stream` awaits `interval.tick()` **twice per frame**: once at the head of the loop body
(`:176`) and once as an arm of the `tokio::select!` at its foot (`:207`). Both consume a tick, so
every frame costs two periods. Measured at `tests/metrics_stream_adversary.rs:468` (gaps
`[1.9998 s, 2.0000 s]` against a 1.5 s ceiling) and `:621` (silent 1.9997 s vs one-ping-per-sample
999.75 ms, **ratio exactly 2.00**), `EXIT_STATUS=101`.

Reachability: every metrics stream. `router_for` (`app/routes.rs:54`) → `metrics_stream` →
`run_stream`; consumer `Client::metrics_stream` (`crates/b10x-substrate-sdk/src/transport.rs:150`).
`contracts/substrate-wire/0.15.0/operations.json` gates the route on the machine fact
`{"latest_wins":true,"replay":false,"sample_interval_ms":1000}`;
`website/docs/guides/storage-and-metrics.md:135` says "approximately once per second";
`website/docs/guides/run-a-command.md:156` tells a UI to pace on "the advertised interval".

Origin **pre-existing**: `git show 617bbed:crates/substrate-daemon/src/app/metrics.rs` has the same
two `interval.tick()` awaits; the unit's only change inside `run_stream` is
`Duration::from_secs(5)` → `policy.send_timeout`.

The second case is what makes the diagnosis unarguable rather than inferred: a control frame cancels
the `select!`'s tick arm, so a noisy client gets exactly the advertised 1 s and a silent one gets 2 s
from the same server over the same exec.

**F2 — `MetricsStreamPolicy` restates five `EventStreamPolicy` values and two of them are pinned.**
`app/metrics.rs:32`. `max_input_bytes`, `max_output_bytes`, `write_buffer_bytes`, `send_timeout` and
`lifetime` are duplicated and equal today; the only pin is
`EventStreamPolicy::production().streams_per_subject` at `metrics.rs:760` and `:788` — 2 fields of 7.
No case is possible: the four have no visibility modifier, so they are private to `app::events` and
no module outside it can read them. Nothing reaches it today; the seam opens on the next edit to
`events.rs`.

**F3 — the 1 h lifetime is wired and unobservable.** `app/metrics.rs:152`.
`tokio::time::timeout(policy.lifetime, session)` is reached by no test, and cannot be:
`lifetime` is private to `app::metrics` and `MetricsStreamPolicy` has no test constructor. A
`start_paused` probe is not honest here — with the 5 s send deadline live, an auto-advancing clock
fires the deadline instead of the lifetime. Any stream open past 1 h reaches it; the exec timeout is
24 h, so this is the ordinary long-run case, and it ends by dropping the future — no close frame, no
code. Fix named: call the already-`pub(super)` `enforce_event_stream_lifetime`
(`app/events.rs:629`) and add the metrics twin of
`app/tests.rs:event_stream_lifetime_cancels_session_and_recovers_permits`.

**F4 — the class check is weaker than its docstring.** `app/metrics.rs:824`. One non-recursive
`read_dir` over `src/app/`, so `src/app.rs`, `src/hosted.rs`, `src/tls.rs` and `src/runtime.rs` are
outside it; it matches the literal `.on_upgrade(`, so `hyper::upgrade::on` is invisible; and it
accepts `.max_frame_size(` anywhere in the raw 800 bytes before the call, which a comment satisfies.
Not constructed — the demonstration is a fourth `on_upgrade` under `src/app/`, an implementation
file, outside the adversary's charter.

**F5 — a new client-visible refusal on a documented route, undocumented.**
`website/docs/guides/storage-and-metrics.md:133`. The acceptance says "one more than a *published*
per-subject cap". `metrics.stream-capacity` and the number 4 appear only in daemon source
(`metrics.rs:25`, `:45`). Repo-wide grep: `metrics.stream-capacity` at `metrics.rs:25` and `:778`
only; `event.stream-capacity` at `events.rs:256` only. `contracts/substrate-wire/0.15.0/refusals.json`
is `b10x.substrate-session-refusals.v1`, 36 rows, every prefix `session` — **so the implementor is
right that no bundle byte was owed**. The SDK surfaces it as `SdkError::Refusal { code }`, so a
caller can act on it once it knows it exists.

## Attacked, could not break

- The acceptance itself, re-driven from an independent fixture: 4 upgrade, the 5th is `429` /
  `metrics.stream-capacity` / `exhausted` / `retriable` / address `stream`.
- Permit return after an abnormal end — a 1 025-byte client frame kills the stream and all 4 permits
  come back. The frame ceiling fires; the structural claim is now behavioural too.
- **The u3 class**, carried across from another unit in this wave: 12 handshakes written then dropped
  unread, full capacity returns.
- **Metrics and event caps are independent — this is the mutant the unit's own three cases miss.**
  `metrics_stream` wired to `app.event_stream_limits` keeps all three of them green.
- Outgoing frame truncation by the new 1 KiB ceiling: `max_frame_size`/`max_message_size` are
  read-side only in tungstenite 0.29, so a server frame cannot be cut by them.
- Ping-flood amplification, measured not argued: the loop consumes one control frame per sample
  period, so a flooding client gets 1 pong/s, not more.
- `EventStreamLimits` accounting cannot overcount across the remove-then-reacquire race.
- The `NoDriver` fake's `unreachable!()` is not vacuous: a driver call panics the request task and
  the unit's own cases go red.

```findings
- file: crates/substrate-daemon/src/app/metrics.rs
  line: 176
  category: contract-drift
  severity: warning
  verdict: needs-revision
  origin: pre-existing
  message: >-
    run_stream awaits interval.tick() twice per frame (:176 and the select! arm at :207), so the
    metrics stream delivers one sample every 2.000s against the 1000ms its released capability
    predicate and both guides advertise; measured at tests/metrics_stream_adversary.rs:468 (gaps
    1.9998s/2.0000s) and :621 (silent 1.9997s vs one-ping-per-sample 999.75ms, ratio 2.00), and the
    same two ticks are present at base 617bbed.
- file: crates/substrate-daemon/src/app/metrics.rs
  line: 32
  category: contract-drift
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: >-
    MetricsStreamPolicy duplicates five EventStreamPolicy values and only streams_per_subject and
    global_streams are pinned by a case, and the other four are private to app::events so no test
    outside that module can compare them; the values agree today, so the drift is latent and cannot
    be made red without widening their visibility or reusing EventStreamPolicy.
- file: crates/substrate-daemon/src/app/metrics.rs
  line: 152
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: >-
    the 1h lifetime added at :152 is reached by no case and cannot be reached by one, because
    lifetime is private to app::metrics and MetricsStreamPolicy has no test constructor; routing it
    through the pub(super) enforce_event_stream_lifetime at app/events.rs:629 would make it
    observable the way the event stream's already is.
- file: crates/substrate-daemon/src/app/metrics.rs
  line: 824
  category: mutant
  severity: note
  verdict: needs-revision
  origin: introduced
  message: >-
    the class check scans src/app/ with one non-recursive read_dir and matches the substring
    ".max_frame_size(" in the raw 800 bytes before ".on_upgrade(", so an upgrade in src/app.rs,
    src/hosted.rs, src/tls.rs or src/runtime.rs, one written as hyper::upgrade::on, or one whose
    bound appears only in a comment all pass it; not constructed, because the demonstration is a
    fourth upgrade under src/app/ and that is an implementation file.
- file: website/docs/guides/storage-and-metrics.md
  line: 133
  category: contract-drift
  severity: note
  verdict: needs-revision
  origin: introduced
  message: >-
    the unit adds a client-visible 429 metrics.stream-capacity to a route this guide documents, and
    neither the cap of 4 nor the refusal code appears anywhere outside daemon source; 0.15.0
    refusals.json is confirmed session-only (36 rows, all session.*) so no bundle byte was owed, but
    the acceptance's word "published" is satisfied by nothing a client can read.
```
