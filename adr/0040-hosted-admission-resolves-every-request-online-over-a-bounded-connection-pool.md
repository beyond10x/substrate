---
status: accepted
date: 2026-10-23
---

# ADR 0040: hosted admission resolves every request online over a bounded connection pool

Extends [ADR 0026](0026-hosted-admission-resolves-opaque-identity-authority.md). ADR 0026 decides
that every production request resolves its credential online at Identity and that no stale
authority is used. It does not say how the resolver reaches Identity. This ADR keeps the online
resolution and decides the transport.

## Context

Until this decision, each hosted request opened a fresh TCP connection to Identity, completed a TLS
1.3 handshake, sent one HTTP/1.1 request carrying `Connection: close` and closed the connection.
Request latency carried a TCP and TLS handshake, and Identity accepted one TLS connection per
Substrate request.

Two remedies were open. A per-credential authority cache, bounded by the authority's own `exp`,
would cut Identity resolutions; it would also let a revoked credential keep working until its cached
authority expired, up to five minutes, which is the stale authority ADR 0026 excludes. A keep-alive
connection pool removes the handshake and leaves the trust posture unchanged.

## Decision

1. **Every hosted request still resolves online.** Substrate caches no authority document, no
   credential and no resolution result, so revocation latency stays zero: a revocation Identity has
   completed applies to the next request. The published fact `stale_authority: never-used` holds
   unchanged.

2. **The resolver reuses connections through a bounded keep-alive pool.** It sends HTTP/1.1 without
   `Connection: close`, over the same TLS 1.3 client, the same explicit CA roots, the same exact
   origin and server name, with no proxy and no redirect. The bounds:

   | bound | value |
   |---|---|
   | idle connections held | at most 8 |
   | idle timeout | 30 s; an older idle connection is closed, never used |
   | resolution deadline | 5 s per request, pool checkout included (unchanged) |
   | response bound | 64 KiB (unchanged) |

   The pool bounds idle connections only. Concurrent resolutions beyond the idle connections open
   fresh connections, as before.

3. **A connection is reused only after a clean exchange.** It returns to the pool only when Identity
   answered `200` with an `application/json` body that was read completely within the bound and
   parsed as an authority document, and the response did not ask to close the connection. Any
   transport error, deadline expiry, other status, wrong content type, oversized or malformed body,
   or `Connection: close` response closes it.

4. **One fresh retry for a stale pooled connection.** If a connection taken from the pool fails
   before Identity answers (the peer closed it while it was idle), the resolver retries once on a
   fresh connection inside the same 5 s deadline. The resolution request is a read-only `GET`, so the
   retry admits nothing that one fresh request would not. A failure on a fresh connection is
   `auth.authority-unavailable`, as before.

## Consequences

The per-request cost of hosted admission is one HTTP round trip to Identity on a reused connection.
A TCP and TLS handshake is added only when no idle connection is available, after an idle timeout,
or after a connection was closed under decision 3. Identity's resolution load still equals the
Substrate hosted request rate; its connection rate falls to the rate at which the pool runs dry.

Nothing on the wire changes: the four authentication refusals, the hosted admission document and
every bundle stay as they are. The acceptance branch "at most one Identity resolution per
credential lifetime" is not taken; reducing resolutions would need a cache and a stated revocation
latency, which is a new decision against ADR 0026, not a refinement of it.
