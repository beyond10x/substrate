---
format: aep.planning-md/1
id: story:network-session-authority
kind: story
status: implemented
title: Network session transport over TLS with single-use proof-bound authority
summary: 'Design 05 decisions 3-4: at most 60 s, redeems once, channel-bound; reconnect is a fresh authority; only the Unix socket serves sessions today.'
owner: substrate
tags:
- daemon
- wave/remote-foundation-01
- wire
relations:
- decomposes: epic:byte-plane-completion
- depends_on: story:production-tls-control-listener
- depends_on: story:hosted-trust-envelope-admission
revision: 10
---
# Story: Network session transport over TLS with single-use proof-bound authority

## Outcome

A hosted client attaches to a raw-pipe or PTY session over WSS only with an authority that lives at most 60 seconds, redeems exactly once, and proves both its ephemeral Ed25519 key and the exact TLS channel. A reconnect always needs a new session and a fresh authority.

## Design closure

Accepted ADR 0027 and `docs/design/14-network-session-authority.md` close the protocol. A hosted-only mint route takes the client public key and returns a one-time bearer outside the durable operation ledger. The attach upgrade carries the authority id, bearer, timestamp and exporter-bound signature in bounded headers so named HTTP refusals happen before upgrade. The store retains only the bearer verifier, public key, expiry and redemption outcome and consumes authority plus attachment right in one transaction.

This replaces the earlier first-WebSocket-frame draft: a frame arrives after HTTP upgrade and therefore cannot produce its promised HTTP 401/409 answer. It also avoids returning secret authority bytes from the replayable session-start operation.

## Acceptance

1. A successor bundle adds the hosted mint route, exact header and transcript vocabulary, 60-second lifetime, 10-second proof skew, four-authority cap, and the four named refusal codes while leaving earlier bundles unchanged.
2. The hosted TLS path mints only for a ready session owned by the verified Identity scope and requiring `exec`; Unix never mints and keeps kernel-authenticated attachment compatibility.
3. The TLS attach path verifies the exact channel exporter and bound Ed25519 key before upgrade, attachment capacity, or durable claim. Authority redemption and the one attachment right are consumed in one `IMMEDIATE` transaction.
4. Missing, expired, replayed, malformed, and wrong-channel authority fail by name. The development static-bearer TCP route set serves neither mint nor attach and binds only on loopback.
5. Tests prove at-most-60-second expiry with paused time, exact single redemption, wrong-channel refusal, concurrent attachment refusal, Unix compatibility, TLS raw-pipe operation, and absence of authority, signature, key, and transcript bytes from captured diagnostics.
6. `bash scripts/gate.sh` and `bash scripts/delegated-lane.sh` pass, and the planning store validates.

## Out of Scope

Identity implementation and login, credential storage, product policy, resumable attachments, TLS-terminating proxies, and remote SDK automation.
