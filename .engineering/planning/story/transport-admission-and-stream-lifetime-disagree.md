---
format: aep.planning-md/1
id: story:transport-admission-and-stream-lifetime-disagree
kind: story
status: draft
title: Transport admission and stream lifetime disagree
scope:
- confidence: cited
  path: crates/substrate-daemon/src/app/events.rs
- confidence: cited
  path: crates/substrate-daemon/src/app/metrics.rs
- confidence: cited
  path: crates/substrate-daemon/src/app/sessions.rs
- confidence: cited
  path: crates/substrate-daemon/src/runtime.rs
revision: 5
---
# Story: Transport admission and stream lifetime disagree

## Context

Closing review finding 4 made an upgraded WebSocket keep the transport permit that admitted it, so
the connection budget counts it for as long as it lives. It does **not** carry the transport's
`connection_lifetime` across the upgrade. The two bounds disagree by 12×:

| bound | value | where |
|---|---|---|
| transport `connection_lifetime` | 5 min | `crates/substrate-daemon/src/runtime.rs` |
| event stream lifetime | 1 h | `crates/substrate-daemon/src/app/events.rs` |
| metrics stream lifetime | 1 h | `crates/substrate-daemon/src/app/metrics.rs` |
| pipe attachment lifetime | 1 h | `crates/substrate-daemon/src/app/sessions.rs` |

So a transport slot is occupied for the stream's hour, not the transport's five minutes.

**Measured during the 2026-09-04 wave B adversarial pass**
(`review-result:adversary-waveb-u2-pass-1`): four identities, each **inside** its published
per-subject cap of 4, fill one source address's 16 transport slots and the next caller from that
address is refused at accept. `TcpConnectionLimits` eviction requires
`available_permits() == per_source`, so the source entry is **non-evictable** for that hour rather
than merely occupied.

Over TCP and TLS the scope is the source address, so behind a shared-address proxy distinct
authenticated identities share one counter and can deny each other. Over the unix listener the scope
is the uid, where a caller can only deny itself.

The published per-subject caps and the per-source transport bound are mutually inconsistent: callers
staying inside their own documented limits can exhaust a bound they are not told about.

## Acceptance

A caller that stays inside every published per-subject stream cap cannot exhaust another caller's
transport admission, and whatever bound does apply to an upgraded connection is published where a
client can read it.

## Notes

**This is a design decision, not a correction.** It needs a design document or an ADR before code
under invariant 8, because it changes what the transport budget means. Three shapes were considered
during the wave and none was taken:

- carry the transport deadline into the upgraded task — **rejected during the wave**: all three
  stream lifetimes are 1 h and published, so enforcing a 5 min transport bound would cut every
  stream at five minutes, breaking a contract to fix a bound;
- raise or re-scope the per-source bound so it cannot be exhausted by callers inside their own caps
  — the number of subjects behind one source address is unbounded, so a constant does not close it;
- give upgraded connections a budget separate from fresh-connection admission — reintroduces the
  question finding 4 answered, and needs the hand-off to be atomic so no window exists where neither
  budget counts the socket.

Two cases pin today's behaviour and name this story:
`an_upgraded_stream_returns_its_transport_slot_at_the_connection_lifetime` and
`upgraded_streams_cannot_hold_a_shared_source_address_past_the_connection_lifetime`
(`crates/substrate-daemon/src/app/tests.rs`). **They go red when this story lands. Invert them,
never relax them** — and they were checked to still discriminate: both go red again if the admission
is released at the upgrade, so they guard finding 4's fix as well as pinning this question.
