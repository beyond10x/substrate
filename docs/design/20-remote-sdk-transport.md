# Design 20: the Rust SDK uses one verified remote transport

**Status:** accepted as [ADR 0029](../../adr/0029-the-remote-sdk-shares-one-verified-https-wss-transport.md) ·
**Date:** 2026-09-01

## Problem

The Rust SDK is currently a Unix-socket wire client. The daemon now has a production TLS 1.3
listener, per-request hosted Identity admission, and one-use key/channel-bound authority for WSS
session attachment. A remote SDK must consume those released seams without adding a second trust
profile, disabling certificate checks, persisting credentials, or weakening mutation recovery.

## Decisions

`ClientBuilder` retains `unix_socket` and adds one mutually exclusive remote configuration. Remote
configuration requires an exact `https://` origin, one bounded PEM CA bundle, an explicit expected
DNS server identity, and an asynchronous access-token provider. There is no system-root default,
plaintext fallback, redirect, proxy, or certificate-verification bypass. The endpoint host is the
TCP destination and HTTP Host authority; the separately supplied DNS identity is what rustls
verifies, so address overrides do not weaken server authentication.

The provider returns an opaque, bounded `identity_access_v1_` credential for each request. It is
called again with a refresh reason only after the daemon returns a named hosted-auth 401. That
response occurs before durable admission, so the SDK may retry the same request bytes and the same
operation id once. A transport failure keeps the existing ledger-first recovery algorithm: query
the original operation id, replay the byte-identical mutation once only when it is absent, and
otherwise return `UnknownOperation`. Credentials and provider errors never enter SDK error text.

HTTP and WebSocket connections are fresh TLS 1.3 connections made from one immutable transport
configuration. Every response or upgrade verifies the promoted contract name and digest. Event and
metrics streams carry the same hosted bearer and preserve their existing typed gap semantics.

A remote `PipeSession::attach` generates a fresh ephemeral Ed25519 key, mints a hosted attachment
authority with that public key, opens a new WSS connection, exports the exact accepting TLS channel,
and signs the frozen session-authority transcript. The four proof headers and hosted bearer are
sent only on that upgrade. The SDK retains neither bearer after the attempt nor a reconnect handle;
another attach creates fresh authority, and the daemon's one-attachment rule decides the outcome.
Unix attachment remains authority-free and source-compatible.

## Failure handling

- Invalid endpoint, roots, or expected identity fail before a network request.
- Unknown roots, name mismatch, expired certificates, TLS downgrade, plaintext and redirects are
  transport failures and never become application admissions.
- Missing, malformed, expired, revoked or refreshed-to-invalid hosted credentials remain named
  daemon refusals. Provider failure is a non-secret `TokenUnavailable` error.
- A lost mutation response preserves its original operation id across token refresh and recovery.
- A WSS disconnect is terminal for that session attachment; no authority is replayed.

## Compatibility and evidence

This adds SDK-owned Rust API only. It adds no route, schema, refusal, capability or event and
therefore cuts no successor contract bundle. Unix and managed-daemon constructors keep their
existing behavior. Shipped-daemon tests use a real private CA, production TLS listener and hosted
Identity resolver to prove successful HTTPS and WSS operation plus root, name, credential rotation,
ambiguous mutation, event-gap and session-authority negatives.
