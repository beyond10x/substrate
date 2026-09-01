---
status: accepted
date: 2026-09-01
---

# ADR 0027: network session authority is key- and channel-bound

## Context

ADR 0024 gives hosted callers a TLS 1.3 channel and ADR 0026 derives their Substrate subject and
route scopes from Identity. Neither fact authorizes one remote client to attach to a particular
raw-pipe or PTY session. Design 05 requires an attachment authority that expires within 60 seconds,
redeems once, binds to the attaching channel, and is never resumed after disconnect.

A bearer alone can be stolen and replayed. A TLS exporter alone proves only that the presenter is
on some TLS channel, because both endpoints of a thief's new connection derive a matching exporter.
Returning a bearer from the durable session-start operation would also force secret bytes into its
replayable operation result. Finally, redemption in the first WebSocket frame cannot produce the
named HTTP refusal status because the upgrade has already completed.

## Decision

Hosted clients mint attachment authority separately at
`POST /v1/pipe-sessions/{session_id}/attachment-authorities`. The request supplies an Ed25519 public
key. The response returns one random bearer, its authority id, and an expiry no more than 60 seconds
after mint. Minting dispatches no driver work and is not a durable operation. The store records only
the bearer's SHA-256 verifier, public key, expiry, and redemption state in a table separate from the
session resource. Authority bytes never enter resources, operations, events, URLs, logs, metrics,
or diagnostics.

The attaching client derives 32 exporter bytes from its TLS connection with label
`EXPORTER-Substrate-Session-Authority-v1`, signs a closed transcript containing the authority id,
exporter, and Unix-millisecond timestamp, and supplies the id, bearer, timestamp and signature in
bounded `x-substrate-session-*` HTTP headers on the WebSocket upgrade. These are distinct from the
Identity `Authorization` header. The daemon derives the exporter from that exact
server-side TLS connection and verifies the proof before upgrading or acquiring attachment
capacity. Hosted Identity still supplies the principal and exact `exec` route scope; the attachment
authority supplies no identity.

The verified authority is consumed in the same SQLite `IMMEDIATE` transaction that consumes the
session's one durable attachment right. A second redemption cannot win a transaction gap. Losing
the WebSocket remains terminal under ADR 0008, so reconnect means a new session and a new authority,
not resumption.

The hosted TLS router serves mint and proof-bound attach. The Unix router keeps authority-free
attachment based on kernel peer credentials and does not mint network authority. The development
static-bearer TCP router serves neither route and is restricted to loopback binding. Future routes
are assigned to an explicit transport set when registered.

The wire names `session.authority-absent`, `session.authority-expired`,
`session.authority-redeemed`, and `session.authority-unbound`. It fixes a 60-second lifetime ceiling,
10-second proof timestamp skew, four live authorities per session, exact bounded header shapes, and
the exporter label and signed transcript. A successor contract bundle adds the mint route and
preserves every predecessor route.

## Consequences

Possession of an authority bearer is insufficient to attach: redemption also needs the bound client
key on the exact TLS channel. Replay, expiry, wrong-channel proof, insecure transport, and missing
material fail by name before process bytes flow. The cost is one ephemeral Ed25519 key pair per
mint, a small durable authority table, and a client implementation that can access TLS exporters.

Reverse proxies that terminate TLS cannot preserve this binding and are not supported on the byte
plane. SDK automation, credential storage, Identity login, resumable sessions and product policy
remain outside this decision.
The attaching client derives 32 exporter bytes from its TLS connection with label
