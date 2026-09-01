# Design 14: network session authority

**Status:** accepted via ADR 0027 · **Date:** 2026-09-01

## Context

[Design 05](05-streams-sessions-and-endpoints.md) requires a network session authority that lives
for at most 60 seconds, redeems once, binds to the attaching channel, and is replaced rather than
resumed after disconnect. ADR 0024 now provides the TLS 1.3 transport and ADR 0026 provides hosted
Identity admission. The Unix socket already authenticates its peer through kernel credentials and
keeps its authority-free attachment path.

The earlier draft put redemption in the first WebSocket frame while assigning HTTP status codes to
redemption failures. Those cannot both be true: after the upgrade, an application frame cannot
change the completed HTTP response. It also proposed returning authority bytes from the durable
session-start operation, which would either persist the bearer in the operation result or make a
replay return different bytes. Both violate the existing durable replay boundary.

## Closed design

### Mint separately from durable session creation

`POST /v1/pipe-sessions/{session_id}/attachment-authorities` is a hosted-only, non-operation route.
Its closed request carries one base64url Ed25519 public key. The daemon confirms the session is
ready and owned by the authenticated scope, creates 32 random bytes, stores only their SHA-256
verifier with the key and expiry, and returns the authority once. The response carries an authority
id, the bearer, and an expiry no more than 60 seconds after mint.

Minting dispatches no driver work, so it does not create an operation. It is deliberately not
idempotent: retry mints another independently bounded authority. A fixed per-session cap bounds
unexpired authorities; expiry cleanup and session terminalisation delete them. Authority bytes are
never placed in the session resource, operation result, event, URL, log, diagnostic, or metric.

### Prove the attaching TLS channel before upgrade

The client exports 32 bytes from its TLS 1.3 connection with label
`EXPORTER-Substrate-Session-Authority-v1` and no context. It signs the closed transcript
`substrate.session-authority.v1`, authority id, exporter bytes, and decimal Unix-millisecond
timestamp with the private half of the key supplied at mint.

The WebSocket upgrade carries four bounded `x-substrate-session-*` headers: the authority id, bearer, timestamp, and
base64url Ed25519 signature. The daemon derives the exporter from its side of that exact TLS
connection, validates the timestamp and signature, and only then upgrades. A bearer stolen without
the client key cannot redeem; a proof copied from another TLS channel verifies against different
exporter bytes and fails.

The exporter reaches the handler only as a per-connection extension. Hosted Identity middleware
still supplies the subject and `exec` scope; the session authority names no principal and cannot
override it.

### Consume atomically with the attachment right

The store keeps authority metadata in a separate table keyed by deployment, subject, session and
authority id. The bearer verifier, public key, expiry and optional redemption instant never enter
`resource_json`. `claim_pipe_session_attachment` consumes a verified, unexpired authority in the
same `IMMEDIATE` transaction that consumes the session's one attachment right. A second redemption
therefore cannot win a separate transaction window.

Malformed, absent, expired, or channel-unbound proof is rejected before the in-memory permit and
before the durable claim. A correctly proved but already redeemed authority is reported by the
transaction. The terminal attachment semantics from ADR 0008 remain: losing an attachment does not
resume it, and a reconnect needs a new session and a newly minted authority.

### Route sets are explicit

The hosted TLS router serves mint and attach. The Unix router serves attach without network
authority and does not serve mint. The development static-bearer TCP router serves neither mint nor
attach; its plaintext posture cannot carry session authority confidentially. The remaining session
observation and lifecycle routes stay available there.

Every future route is therefore classified into an explicit transport set rather than silently
appearing on every listener. Development TCP is also restricted to loopback binding, because
`--tcp-private-overlay` is an operator assertion rather than a transport guarantee.

### Refusals and limits

The successor contract publishes:

| condition | status | code |
|---|---:|---|
| hosted mint or attach without the required material | 401 | `session.authority-absent` |
| authority expired | 401 | `session.authority-expired` |
| authority already redeemed | 409 | `session.authority-redeemed` |
| malformed bearer/proof or proof for another channel | 401 | `session.authority-unbound` |
| mint or attach on an insecure listener | 404 | route absent |

The lifetime ceiling is 60 seconds, the proof timestamp skew is 10 seconds, authority credentials
and signatures are individually bounded, and at most four live authorities may exist for one
session. Bounds are constants in `substrate-wire` and are pinned by the bundle checker.

## Contract and evidence

`substrate-wire/0.14.0` directly succeeds `0.13.0`, preserves its 33 routes, and adds the authority
mint route. Earlier bundle bytes remain unchanged. Its checker pins the route, request and response
shapes, header vocabulary, exporter label, limits, and refusal codes.

Tests prove plaintext route absence, at-most-60-second expiry with paused time, exact single
redemption, wrong-channel refusal, second concurrent attachment refusal, Unix compatibility, and
that captured diagnostics contain no authority, signature, public key, or transcript marker. The
delegated lane drives a real confined raw-pipe session through the hosted TLS path.

## Out of scope

Identity login and credential storage, browser sessions, product policy, resumable attachments,
TLS termination proxies, and client SDK automation are separate work. The remote SDK story consumes
this protocol after it is released.
