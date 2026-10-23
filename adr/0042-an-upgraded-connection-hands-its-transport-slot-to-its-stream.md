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
| session attachment | 1 per session, none per subject | 32 | `session.attachment-capacity` |

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
4. **A session attachment has a per-subject cap of 16**, refused `session.attachment-capacity`
   (429) like the deployment-wide cap. The hand-over removed the only bound that stood between one
   subject and all 32 attachments. Before it, that bound was the transport's, and it was never a
   per-subject one: on the unix socket the subject is `local:<uid>` and the per-uid transport cap is
   32, equal to the attachment cap, so one uid could already hold all 32; over TCP and TLS it was 16
   per source address, so one subject behind one address held at most 16, and one subject behind
   two addresses held all 32. The cap keeps the TCP figure and applies it per subject on every
   listener, which tightens the unix socket from 32 to 16. One subject can then hold at most half of
   the deployment's attachments, and a second subject always finds one free unless a third holds
   it. The cap is counted beside the per-session and global caps in `PipeAttachmentLimits`
   (`crates/substrate-daemon/src/app/sessions.rs`), taken before the `101` and returned when the
   attachment ends, on every path.
5. **The metrics stream route refuses before the upgrade when the snapshot does not publish
   `metrics.stream`**, with `exec.metrics-unserved` (501, class `unserved`). The route already
   declared `metrics.stream` as its capability predicate in the bundle, and the daemon did not
   consult it: a host that published neither `metrics.stream` nor `metrics.stream-bounds` still
   upgraded and served a capped stream. `exec.metrics-unserved` is the published code for
   "this host has not proved execution resource counters" (the `measurements` refusal of
   `exec.start`), which is what an absent `metrics.stream` states; no code is added. The check
   runs after the query is parsed and before the stream permit is taken.
6. **The caps, the lifetime and the refusal code of each family are capability facts**, which a
   client reads from `GET /v1/machine` before it opens a stream:

   | fact | members |
   |---|---|
   | `events.stream-bounds` | `per_subject: 4`, `global: 64`, `lifetime_ms: 3600000`, `refusal: "event.stream-capacity"` |
   | `metrics.stream-bounds` | `per_subject: 4`, `global: 64`, `lifetime_ms: 3600000`, `refusal: "metrics.stream-capacity"` |
   | `sessions.attachment-bounds` | `per_subject: 16`, `per_session: 1`, `global: 32`, `lifetime_ms: 3600000`, `refusal: "session.attachment-capacity"` |

   `refusal` names the code for a stream over its family's caps. The one exception is the
   per-session attachment cap: a second attachment to one session keeps its existing, more
   specific refusal, `session.already-attached`.

   Each value is a `substrate_wire` constant. The daemon's stream policies use the same constants,
   and the host probe publishes them: `events.stream-bounds` always, `metrics.stream-bounds`
   exactly when `metrics.stream` is published, and `sessions.attachment-bounds` exactly when exec
   is served, because a session is an exec. Bundle `0.23.0` declares each fact as a closed object
   of constants, and `cargo xtask check-bundle` holds every constant to its `substrate_wire`
   value.
7. **Stream capacity is published as a fact, not in `refusals.json`.** `refusals.json` is the
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
- One subject holds at most 16 of the deployment's 32 session attachments (decision 4). A unix
  client that attached more than 16 sessions at once under one uid is now refused at the 17th; no
  published bound said it could.
- The event and metrics streams keep their per-subject caps and no per-source one, and that is
  accepted: sixteen subjects behind one source address, each inside its cap of four, hold all 64
  event streams (or all 64 metrics streams) for the hour, and every other caller is refused the
  family's `429` until one ends. The streams are a deployment-wide budget by design; bounding the
  subjects behind one address is admission's concern, not a stream cap's, and no code changes for
  it here.
- A host that does not publish `metrics.stream` now answers `501 exec.metrics-unserved` on the
  metrics stream route where it used to answer `101` (decision 5). A client that followed the
  route's published capability predicate never sent that request.
- A client that validates snapshots against `0.22.0` rejects a snapshot that publishes one of the
  three facts until it moves to `0.23.0`, as with every new fact on the closed facts object.
