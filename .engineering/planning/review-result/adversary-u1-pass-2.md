---
format: aep.planning-md/1
id: review-result:adversary-u1-pass-2
kind: review-result
status: active
title: Adversary pass 2, unit u1 (metrics stream caps)
relations:
- reviews: story:metrics-streams-are-bounded
revision: 1
---
# Adversary pass 2 — unit u1, `story:metrics-streams-are-bounded`

Agent type `adp:adversary`. Worktree `wt-d5a923d7143d`, branch `impl/metrics-streams-bounded`,
HEAD `41aa112`, base `617bbed`. Report as returned.

```
unit: u1 — story:metrics-streams-are-bounded
verdict: red
cases: executed 139→142, red 2
origin: introduced 2 / pre-existing 2 / undecided 0
wrote-outside-worktree: 4 paths
needs-coordinator: no
```

## Cases added

182 insertions, **0 deletions**, appended below a marker line; no existing line in the pass-1 file
touched.

| line | case | result |
|---|---|---|
| 718 | `a_metrics_control_frame_flood_earns_the_close_the_event_stream_gives` | **red** |
| 762 | `a_metrics_data_frame_earns_the_named_close_the_event_stream_gives` | **red** |
| 793 | `the_first_metrics_sample_is_immediate_and_the_second_is_one_interval_later` | green |

```
thread 'a_metrics_control_frame_flood_earns_the_close_the_event_stream_gives' panicked at
crates/substrate-daemon/tests/metrics_stream_adversary.rs:716:5:
assertion `left == right` failed: the event stream spends a client's 120 control frames per window
and then closes 1008 (tests/websocket.rs:354); the metrics stream answered 121 pongs to 121 pings in
6.00158252s and ended as StillOpen
  left: StillOpen
 right: Close(1008)

thread 'a_metrics_data_frame_earns_the_named_close_the_event_stream_gives' panicked at
crates/substrate-daemon/tests/metrics_stream_adversary.rs:760:5:
assertion `left == right` failed: a 512 byte data frame is half the declared 1024 byte ceiling, and
the metrics stream ended as Eof — the same ending the 1 025 byte frame at line 527 gets, so that
case does not observe the ceiling; the event stream answers this frame with close 1003
(tests/websocket.rs:314)
  left: Eof
 right: Close(1003)
```

Suite: **exit 101**, 142 executed, 140 passed, 2 failed. Every other target green. `139` corroborated
two ways: the implementing state's number, and a re-run with the three new cases deselected
(`5 passed; 3 filtered out`, exit 0).

## Findings

**F1 — the metrics stream has no control-frame budget.** `metrics.rs:214`, blocker,
**pre-existing**. 121 pings answered with 121 pongs, no close frame, stream still open at the 6 s
deadline. The identical flood on the event stream closes 1008
(`tests/websocket.rs:354`, green). `EventStreamPolicy` bounds this with
`max_controls_per_window: 120` / `control_window: 1 min` (`events.rs:52-53`) enforced at
`events.rs:515-527`; `MetricsStreamPolicy` (`metrics.rs:31-39`) restates neither, while its own
docstring (`metrics.rs:26`) claims it holds "the shape `EventStreamPolicy` publishes".
`617bbed` has no `ControlRate` either. Fix named: carry the two fields and run `events::ControlRate`
in `run_stream`'s inner loop, closing 1008 the way `events.rs:521` does.

**F2 — the cadence fix removed the incidental throttle that stood in for F1's missing guard.**
`metrics.rs:214`, blocker, **introduced**. At `617bbed` and `377b115` the `select!` was the last
statement of the outer loop, whose head awaited `interval.tick()`, so **at most one** control frame
was answered per 1 s sample period — pass 1 measured exactly this and filed it under *could not
break*: "a flooding client gets 1 pong/s, not more". `402cb55` wrapped the `select!` in an inner
`loop`, so the route now answers control frames at line rate: a read, a match and a `socket.send`
per frame, on a permit held up to the 1 h lifetime. Same code, ~20× the answered frames in the same
window, and unbounded above that.

**F3 — the oversized-frame case does not observe the frame ceiling, and `metrics.rs:944` says it
does.** `metrics.rs:222`, warning, **pre-existing**. A 512-byte frame — half the declared ceiling,
rejected by no bound — ends the stream as `Eof` through the same `_ => return` arm an oversized
frame takes. Neither sends a close frame; both end by dropping the socket. Pass 1's case accepts
`None | Some(1009)`, so it cannot tell them apart. Consequence: **the mutant
`max_frame_size(usize::MAX)` survives the whole suite**, and the unit's stated limit ("proves the
bounds are declared, never that they fire") has no partner closing it. The guide paragraph this unit
added promises "a client is told when it hits a bound"
(`website/docs/guides/storage-and-metrics.md:139`); the client is told nothing. The sibling route
answers the same frame with close 1003 and a reason (`events.rs:505-511`).

**F4 — one of `41aa112`'s five new assertions is vacuous.** `app/tests.rs:534`, warning,
**introduced**. `assert_eq!(metrics.max_output_bytes, BODY_LIMIT)` compares `BODY_LIMIT` with
`BODY_LIMIT` — `MetricsStreamPolicy::production()` sets `max_output_bytes: BODY_LIMIT`
(`metrics.rs:48`). It cannot fail. `tests.rs:526`,
`assert_eq!(metrics.max_output_bytes, events.max_output_bytes)`, is blind to the same move:
`events.rs:50` is also `BODY_LIMIT`. The other four new comparisons read independent literals in two
independent `const fn`s and **do** bite. The docstring `41aa112` installed at `tests.rs:514-517`
says "every published value is also pinned against its literal" — for `max_output_bytes` there is no
literal and no pin, so moving `app.rs:6` (`const BODY_LIMIT: usize = 2_097_152`, one edit away,
already feeding `routes.rs:36`) moves the metrics write-buffer ceiling with nothing red.

**F5 — the pong reply has no send deadline.** `metrics.rs:219`, note, `INFEASIBLE`.
`socket.send(Message::Pong(bytes)).await` is bare; the sample send at `metrics.rs:196-201` is wrapped
in `tokio::time::timeout(policy.send_timeout, …)`. Not constructible: the sample send fires its 5 s
deadline first in every ordering the adversary could build. The asymmetry is one line and
`policy.send_timeout` is already in scope.

## Attacked, could not break

- **The pass-1 file's integrity — verified clean.** All five case bodies present, all five line
  numbers shifted by exactly −1, and both assertion messages the pass-1 report quoted are
  byte-identical in the committed file. **No assertion was changed, relaxed, renamed or dropped.**
- **`41aa112` leaks nothing.** `mod app;` is private in `lib.rs:3`, so `pub(super)` on an
  `app::events` field is visible in `app` and nowhere else; no re-export.
- **Four of `41aa112`'s five new assertions bite** — two independent `const fn`s with independent
  literals. Only `max_output_bytes` is vacuous (F4).
- **`max_catch_up_pages` / `max_page_items` left private is right** — no metrics analogue.
  `max_controls_per_window` / `control_window` are the seam that commit's reasoning missed (F1);
  they were **already** `pub(super)`, so no widening was needed to close it.
- **The post-fix cadence shape is right at the head of the stream** — first sample < 500 ms after
  the upgrade, second ≥ 500 ms after the first. Neither dropped nor doubled.
- **F1's `tokio::time::interval` enumeration is complete** — repo-wide grep finds exactly the four
  sites named.
- **`the_advertised_sample_interval…` genuinely cannot be red-first**, and `probe.rs:153` uses
  `substrate_wire::RESOURCE_USAGE_SAMPLE_INTERVAL_MS` rather than a literal, so the advertised fact
  cannot desync from what the daemon paces on. Small gap: the case counts *versions*, not
  operations.
- **The masker could not be desynced from outside.** The one candidate found by reading — `'\''` —
  occurs nowhere under `src/`, so it is latent, not live.
- **The guide edit does not reach an Atlas-owned field** — frontmatter is `title`/`description` only
  and was not touched. Agreeing with the unit that reconciliation has nothing to drift.
- **Agreeing that the machine-readable cap is a story, not a line.**

```findings
- file: crates/substrate-daemon/src/app/metrics.rs
  line: 214
  category: contract-drift
  severity: blocker
  verdict: needs-revision
  origin: pre-existing
  message: >-
    the metrics stream answers client control frames with no budget and no close — 121 pings earned
    121 pongs and no close frame at metrics_stream_adversary.rs:718 (exit 101) — while
    EventStreamPolicy bounds exactly this at events.rs:52-53 and closes 1008 (tests/websocket.rs:354),
    and MetricsStreamPolicy at metrics.rs:31-39 restates neither of those two fields despite its
    docstring claiming the shape EventStreamPolicy publishes; reproduces at 617bbed, which has no
    ControlRate either.
- file: crates/substrate-daemon/src/app/metrics.rs
  line: 214
  category: concurrency
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: >-
    402cb55 wrapped the client-frame select! in an inner loop, removing the outer interval.tick()
    that admitted at most one control frame per sample period, so the unguarded control path now runs
    at line rate — 121 frames answered inside one 6 s window against at most 6 in the pre-fix shape,
    on a permit held up to the 1 h lifetime.
- file: crates/substrate-daemon/src/app/metrics.rs
  line: 222
  category: mutant
  severity: warning
  verdict: needs-revision
  origin: pre-existing
  message: >-
    a 512-byte data frame ends the metrics stream as Eof (metrics_stream_adversary.rs:762, exit 101)
    through the same `_ => return` arm an oversized frame takes, so
    an_oversized_client_frame_ends_the_metrics_stream_and_returns_its_permit cannot observe the frame
    ceiling and max_frame_size(usize::MAX) survives the suite — yet metrics.rs:944 names that case as
    the one that observes the bound firing and the guide at storage-and-metrics.md:139 promises the
    client is told which bound it hit.
- file: crates/substrate-daemon/src/app/tests.rs
  line: 534
  category: mutant
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: >-
    assert_eq!(metrics.max_output_bytes, BODY_LIMIT) compares BODY_LIMIT with itself (metrics.rs:48)
    and the cross-policy comparison at tests.rs:526 reads BODY_LIMIT on both sides (events.rs:50), so
    one of the seven bounds is pinned by two tautologies and moving app.rs:6 moves the metrics
    write-buffer ceiling with nothing red, against the docstring at tests.rs:514-517 claiming every
    published value is pinned against its literal.
- file: crates/substrate-daemon/src/app/metrics.rs
  line: 219
  category: judgement
  severity: note
  verdict: needs-revision
  origin: pre-existing
  message: >-
    the pong reply is a bare socket.send().await with no policy.send_timeout while the sample send at
    metrics.rs:196-201 has one, leaving only the 1 h lifetime above a blocked write; not constructible
    because the 5 s sample deadline fires first in every ordering that could be built.
```
