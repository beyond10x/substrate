---
format: aep.planning-md/1
id: review-result:adversary-waveb-u2-pass-1
kind: review-result
status: active
title: Adversary pass 1, wave B unit u2 (transport permit across upgrade)
relations:
- reviews: story:upgraded-connections-keep-their-permit
revision: 1
---
# Adversary pass 1 — wave B unit u2, `story:upgraded-connections-keep-their-permit`

Worktree `wt-e6b4daeb23e6`, branch `impl/upgraded-connections-permit`, HEAD `e9709ff`, base
`0c858f0`. Report as returned.

```
unit: u2 story:upgraded-connections-keep-their-permit
verdict: red
cases: executed 153→155, red 2
origin: introduced 4, pre-existing 0, undecided 0
needs-coordinator: yes — finding 1 has two admissible fixes and only the coordinator picks
```

## Cases added

`crates/substrate-daemon/src/app/tests.rs` +294, one path, no implementation file edited. Under
`src/` because `TransportPermit`, `admitted_service` and `TcpConnectionLimits` are `pub(crate)`.

```
thread '…::an_upgraded_stream_returns_its_transport_slot_at_the_connection_lifetime' panicked at
crates/substrate-daemon/src/app/tests.rs:788:9:
the transport's connection lifetime of 300ms expired more than 16 times over and the upgraded socket
still holds the connection's transport slot; occupancy is bounded by the stream's one-hour lifetime,
not by the transport's, so one caller can hold a source's admission for an hour

thread '…::upgraded_streams_cannot_hold_a_shared_source_address_past_the_connection_lifetime'
panicked at crates/substrate-daemon/src/app/tests.rs:891:9:
the transport's connection lifetime of 300ms expired more than 16 times over and a caller sharing
the source address is still refused at accept; four identities each inside their published
per-subject cap can deny every other caller behind that address for the stream's one-hour lifetime
```

Suite exit 101, 155 executed, 2 failed. Re-run with `--skip upgraded_transport_slot`: 153 executed,
0 failed, exit 0 — the two reds are the adversary's and nothing else regressed. clippy 0, fmt 0.

## Findings

**F1 — the transport's connection lifetime is not carried across the upgrade, and the admission now
is.** `runtime.rs:62`, blocker, `introduced`. One upgraded event stream holds a transport slot for
≥16× the transport's connection lifetime. Four identities inside their published per-subject cap of
4 (`events.rs:49`) fill one source address's 16 slots and deny every co-located caller at accept.
`UnixTransportPolicy::production().connection_lifetime` is 5 min (`runtime.rs:57`); all three stream
lifetimes are 1 h (`events.rs:57`, `metrics.rs:74`, `sessions.rs:72`) — **12× worse occupancy**. Over
TCP and TLS the scope is the source address (`runtime.rs:275`, 16/source), so behind a shared-address
proxy distinct authenticated identities share one counter. `TcpConnectionLimits` eviction requires
`available_permits() == per_source` (`runtime.rs:296`), so a source entry is **non-evictable for an
hour** rather than for a connection.

Origin `introduced`: at `0c858f0` there is no `admitted_service` and no `Extension(permit)`, so no
upgraded task could hold a transport slot and the denial did not exist. The comment at
`runtime.rs:61-62` — "The outer connection lifetime remains the finite bound for idle and upgraded
peers" — is the stated justification for `keep_alive: true`; it predates the unit, and the unit made
its falsity load-bearing.

`NEEDS-CHANGE` rather than `CONFIRMED`, deliberately: **two fixes are admissible and the choice is
not the adversary's.** Carry the transport deadline into the upgraded task, which turns these cases
green; or record the 1 h occupancy as a decision and correct `runtime.rs:61-62` and the `keep_alive`
justification, which inverts them. Either way something must change — the unit's commit message
calls this "measured and not fixed here", the story's Notes label it a code read, the story's
Acceptance does not mention it, and invariant 8 wants a capability change beyond a story's named
decisions recorded before code.

Why the unit's own case cannot see it: `test_policy` sets `connection_lifetime: 7 s`
(`runtime.rs:1353`), the fixture stream lifetime is 30 s, and the case completes in under 5 s — it
cannot distinguish "held for the socket's life" from "held past the transport's bound".

**F2 — `every_upgradeable_connection_publishes_its_transport_admission` claims a scope it does not
have.** `metrics.rs:1363`, warning, `introduced`. The check walks `CARGO_MANIFEST_DIR/src` only,
while `tests/websocket.rs:110-111`, `tests/metrics_stream_adversary.rs:94-95` and
`tests/pipe_session.rs:662-664` each serve the **real production router** with `.with_upgrades()` and
publish no admission. No production path reaches it; the consequence is coverage. Before this pass,
**no production route's `let _transport_admission = …` line had any behavioural coverage** — only the
text check saw them, and the behavioural partner named in the doc drives a fixture route in
`runtime.rs`.

**F3 — the class check is an identifier-name match in both directions.** `metrics.rs:1287`, note,
`introduced`. `drop(transport_admission);` satisfies it while releasing the slot (declared);
hoisting the binding above the chain fails it on correct code (not declared).

**F4 — the metrics test harness now enforces a budget it never had, and refuses silently.**
`metrics.rs:623`, note, `INFEASIBLE`. A refused connection is dropped with a bare `continue`, which
surfaces as an io panic inside `Handshake::open` rather than a named capacity condition. **Nothing
found** reaches >16 concurrent from the existing suite.

## Attacked and could not break

- **The refcount leak.** A three-phase probe — upgrade then client vanishes, route refuses before any
  upgrade, handshake abandoned without reading the 101 — asserted the slot was occupied and then that
  all 16 returned. Green on all three. `axum-0.8.9/src/extract/ws.rs` drops the `on_upgrade` callback,
  and the permit it captures, when `OnUpgrade` errors, so the 0.6.0 twin is not present.
- `let _transport_admission = …` is the first statement of the async block in all three routes, live
  across every await including `sessions.rs`'s post-timeout tail.
- `TcpConnectionLimits` eviction still reads the `OwnedSemaphorePermit`'s clone, so cloning the
  handle cannot make a held source evictable.
- `admitted_service` reaches the handler on all three listeners.

```findings
- file: crates/substrate-daemon/src/runtime.rs
  line: 62
  category: contract-drift
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: >-
    the unit carries the transport admission across a WebSocket upgrade but not the transport's
    connection_lifetime, so a slot is occupied for the stream's 1 h instead of the transport's 5 min
    and four identities inside their published per-subject caps deny every co-located caller at
    accept over a shared source address. Measured red at src/app/tests.rs:799 and :890, exit 101,
    absent at base 0c858f0 where no permit was published into requests, and contradicting the
    comment here that justifies keep_alive: true.
- file: crates/substrate-daemon/src/app/metrics.rs
  line: 1363
  category: judgement
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: >-
    the check documents itself as the rule a listener added later cannot skip but walks src/ only, so
    tests/websocket.rs:111, tests/metrics_stream_adversary.rs:95 and tests/pipe_session.rs:664 serve
    the real production router with .with_upgrades() and no admission, leaving every production
    route's transport-admission hold behaviourally uncovered before this pass.
- file: crates/substrate-daemon/src/app/metrics.rs
  line: 1287
  category: mutant
  severity: note
  verdict: needs-revision
  origin: introduced
  message: >-
    the class check matches the literal identifier transport_admission inside the .on_upgrade(
    argument list, so a deliberate drop() passes it (declared) and hoisting the binding above the
    chain fails it on correct code (not declared).
- file: crates/substrate-daemon/src/app/metrics.rs
  line: 623
  category: boundary
  severity: note
  verdict: needs-revision
  origin: introduced
  message: >-
    the metrics harness now admits from TcpConnectionLimits::production() and drops a refused
    connection with a bare continue, which surfaces to the client as an io panic inside
    Handshake::open rather than a named capacity condition; no existing case reaches more than 5
    concurrent connections against the 16-per-source bound, so nothing was shown to reach it.
```
