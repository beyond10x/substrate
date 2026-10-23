---
status: accepted
date: 2026-10-23
---

# ADR 0042: an upgraded connection hands its transport slot to its stream

Decides `story:transport-admission-and-stream-lifetime-disagree` and
`story:the-metrics-stream-cap-is-machine-readable`. It changes what the transport budget counts and
adds three capability facts to the unreleased bundle `0.23.0`.

## Context

Every listener admits a connection under a transport permit: 128 global and 32 per uid on the unix
socket, 128 global and 16 per source address over TCP and TLS
(`crates/substrate-daemon/src/runtime.rs`, `UnixTransportPolicy::production` and
`TcpConnectionLimits::production`). The connection future runs under a five-minute
`connection_lifetime`. hyper resolves that future when it hands the socket to a WebSocket upgrade,
so the five minutes end at the handshake.

Three routes upgrade: the event stream (`/v1/events/stream`), the metrics stream
(`/v1/metrics/stream`) and the session attachment. Each takes its own permit before it answers
`101`, refuses a caller over its cap with a named `429`, and lives at most one hour:

| family | per subject | per deployment | refusal |
|---|---|---|---|
| event stream | 4 | 64 | `event.stream-capacity` |
| metrics stream | 4 | 64 | `metrics.stream-capacity` |
| session attachment | 1 per session | 32 | `session.attachment-capacity` |

Review finding 4 (`story:upgraded-connections-keep-their-permit`) made every upgraded task keep the
transport permit too, so the transport budget counted a socket that was still serving. That closed
a hole and opened a second one, measured in the 2026-09-04 adversarial pass and pinned since by two
cases in `crates/substrate-daemon/src/app/tests.rs`: four identities, each inside its published cap
of four event streams, hold one source address's sixteen transport slots for the stream's hour, and
every other caller behind that address is refused at accept. `TcpConnectionLimits` cannot evict the
entry, because eviction needs every permit free. The per-subject caps a client is told about and the
per-source bound it is not told about disagree.

The event and metrics caps and their refusal codes were published in daemon source and in a guide,
never in a byte the wire publishes. The session cap was published in the session capability result
only.

Two other shapes were rejected in the story. Carrying the five-minute deadline into the upgraded
task cuts every published one-hour stream at five minutes. Raising or re-scoping the per-source
bound does not close it, because the number of subjects behind one address is unbounded.

## Decision

1. **An upgraded connection hands its transport permit over to the stream it became.** From the
   upgrade on, the socket is counted by its stream's caps (per subject or per session, and per
   deployment) and no longer by the transport's. The transport budget bounds connections that have
   not upgraded, for at most `connection_lifetime`.
2. **The hand-off is atomic in one direction.** The stream permit is taken in the handler, before
   the `101` is sent. The transport permit is released inside the upgraded task, by
   `runtime::hand_over_transport`, which takes a reference to the held stream permit and so cannot
   be called without one. Between the upgrade and that call the socket is counted by both budgets;
   at no instant is it counted by neither. A request that fails to upgrade never releases the
   transport permit early: the permit returns when the connection ends, as before.
3. **Every stream family has a deployment-wide cap.** All three already had one (64, 64 and 32);
   none was added. Upgraded sockets are therefore bounded by 160 per deployment in total, whatever
   the transport admitted.
4. **The caps, the lifetime and the refusal code of each family are capability facts**, which a
   client reads from `GET /v1/machine` before it opens a stream:

   | fact | members |
   |---|---|
   | `events.stream-bounds` | `per_subject: 4`, `global: 64`, `lifetime_ms: 3600000`, `refusal: "event.stream-capacity"` |
   | `metrics.stream-bounds` | `per_subject: 4`, `global: 64`, `lifetime_ms: 3600000`, `refusal: "metrics.stream-capacity"` |
   | `sessions.attachment-bounds` | `per_session: 1`, `global: 32`, `lifetime_ms: 3600000`, `refusal: "session.attachment-capacity"` |

   `refusal` names the code for a stream over its family's caps. The one exception is the
   per-session attachment cap: a second attachment to one session keeps its existing, more
   specific refusal, `session.already-attached`.

   Each value is a `substrate_wire` constant. The daemon's stream policies use the same constants,
   and the host probe publishes them: `events.stream-bounds` always, `metrics.stream-bounds`
   exactly when `metrics.stream` is published, and `sessions.attachment-bounds` exactly when exec
   is served, because a session is an exec. Bundle `0.23.0` declares each fact as a closed object
   of constants, and `cargo xtask check-bundle` holds every constant to its `substrate_wire`
   value.
5. **Stream capacity is published as a fact, not in `refusals.json`.** `refusals.json` is the
   session refusal vocabulary (`b10x.substrate-session-refusals.v1`); a fact sits beside the
   capabilities a client already reads before opening a stream, and states the cap and the code
   together.

## Consequences

- A caller that stays inside every published per-subject stream cap cannot hold a transport slot
  past the handshake, so it cannot deny transport admission to a caller that shares its source
  address or uid. The two pinning cases in `app/tests.rs` are inverted to assert that.
- Finding 4's guarantee changes form: an upgraded socket is still never uncounted, but the budget
  that counts it is its stream's, not the transport's. `runtime::tests` and the crate-wide source
  check in `app/metrics.rs` now require the hand-over call inside every `.on_upgrade(` task.
- A deployment can serve up to 160 upgraded sockets beside 128 un-upgraded connections. That is the
  sum of the published caps, and every term of it is named.
- The session attachment has no per-subject cap. One subject with 32 attachable sessions can hold
  the deployment's 32 attachments; it is published as `global`, and a per-subject attachment cap is
  a separate decision.
- A client that validates snapshots against `0.22.0` rejects a snapshot that publishes one of the
  three facts until it moves to `0.23.0`, as with every new fact on the closed facts object.
