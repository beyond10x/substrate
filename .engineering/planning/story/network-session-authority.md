---
format: aep.planning-md/1
id: story:network-session-authority
kind: story
status: draft
title: Network session transport over TLS with single-use proof-bound authority
summary: 'Design 05 decisions 3-4: at most 60 s, redeems once, channel-bound; reconnect is a fresh authority; only the Unix socket serves sessions today.'
owner: substrate
tags:
- daemon
- wire
relations:
- decomposes: epic:byte-plane-completion
revision: 2
---
# Story: Network session transport over TLS with single-use proof-bound authority

## Outcome

A client on another machine attaches to a session over WebSocket/TLS with an authority that lives
at most 60 s, redeems exactly once, and is bound to the redeeming channel; a reconnect always
needs a fresh one. This is the last phase-4 exit criterion.

## Context

`docs/design/05-streams-sessions-and-endpoints.md` § *V1 decisions* 3–4 fix the authority and the
transport; `architecture/deployment-postures.md` requires TLS/mTLS or a trusted tunnel for any
non-loopback control listener. Today only the owner-permissioned Unix socket serves sessions
(plan 04 § *Slice B*). The static-bearer TCP listener is development-only
(`crates/substrate-daemon/src/main.rs:63-64`, design 06 § 1) and must not gain this route.

## Acceptance

The delegated lane drives a raw-pipe session end to end over TLS from a second network namespace,
and the authority it used cannot be redeemed a second time, after 60 s, or over a different TLS
session.

Evidence that satisfies it, in order:

1. An ADR decides the proof binding (TLS exporter per RFC 5705, or a client key) and records that
   reconnect is a new authority, never a resumed one.
2. A successor bundle adds the network attachment route and the authority-redemption frame; earlier
   bundle bytes unchanged.
3. Failing-first tests: `network_session_listener_refuses_plaintext`,
   `session_authority_redeems_exactly_once`, `session_authority_expires_after_60s` (tokio paused
   time), `session_authority_bound_to_channel`, `second_concurrent_attachment_refused`.
4. The authority value never appears in logs, events or error bodies — a test greps captured
   diagnostics (design 05 § 3, "non-loggable").
5. The development-only TCP listener does not serve the route — a test proves its absence there.

## Out of Scope

The hosted trust-envelope verifier (design 06 § 1, atlas ADR 0015; phase 7): this story issues
the authority from the local bearer subject.
