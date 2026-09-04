---
format: aep.planning-md/1
id: story:hosted-admission-reuses-identity-authority
kind: story
status: draft
title: Hosted admission reuses a resolved Identity authority within its lifetime, or an ADR records why not
summary: 'Every TLS request opens a fresh connection to Identity with Connection: close and no cache (hosted.rs:170-194).'
tags:
- review
relations:
- decomposes: epic:review-2026-09-03-findings
scope:
- confidence: cited
  path: adr/0026-hosted-admission-resolves-opaque-identity-authority.md
- confidence: cited
  path: crates/substrate-daemon/src/hosted.rs
revision: 4
---
# Story: Hosted admission reuses a resolved Identity authority

## Context

Every request on the production TLS listener resolves its credential against Identity with a
fresh TCP connect, TLS 1.3 handshake and HTTP/1.1 request carrying `Connection: close`
(`crates/substrate-daemon/src/hosted.rs:170-194`). Nothing is cached, although the authority
carries `jti`, `exp` and a lifetime bound of 5 min (`hosted.rs:42-43`, `validate_authority`).
Per-request latency includes one Identity round trip, and Identity load equals Substrate request
rate. ADR 0026 chose online resolution over introspection caching; it does not state that
connection reuse or a bounded per-credential cache is excluded.

## Acceptance

Two hosted requests carrying the same credential within its remaining authority lifetime cause at most one Identity resolution, or an ADR records that every request must resolve online.

## Notes

Connection reuse alone (a small keep-alive pool to Identity) removes the handshake without any change to the trust posture. Whichever branch closes the story also states the revocation latency bound, or the per-request cost, on the capability document and in the deployment guide; those statements follow from the branch and are not a third claim.
