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
revision: 3
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

## Scope

Derived 2026-08-30 by `story-scoper`. Every line is **cited** or **inferred**.

- **Primary surface:** `crates/substrate-daemon` — cited; both the listener and the attach route
  live there.
- **Files, cited:** `crates/substrate-daemon/src/runtime.rs:736` (`serve_tcp`), `:856`
  (`require_tcp_bearer`), `src/app/routes.rs:33` (`router`), `src/app/sessions.rs:735`
  (`pipe_session_attach`).
- **Symbols, cited:** `serve_tcp`, `TcpAuthState`, `require_tcp_bearer`, `router`,
  `pipe_session_attach`, `PipeAttachmentPermit`, `PipeAttachmentRefusal`.
- **Also likely, inferred:** `crates/substrate-wire/src/lib.rs:1274`/`:1291` (`PipeClientFrame`,
  `PipeServerFrame`), where an authority-redemption frame would be added;
  `crates/substrate-store/src/schema.rs:154` and `src/sessions.rs:526`
  (`claim_pipe_session_attachment`), where redeem-exactly-once becomes durable;
  `crates/substrate-daemon/src/main.rs:192-222`, the clap flags beside the `--tcp-*` group.
- **Dependency, cited:** the workspace has **no** TLS crate — `rustls`/`tokio-rustls` grep to
  nothing — so the dependency and its configuration surface are unmade.
- **Confidence:** **medium.** The crate and the two code sites are cited and certain, but there is
  no accepted ADR (`adr/` holds 0001–0014) and `docs/design/05-streams-sessions-and-endpoints.md:65`
  points at `architecture/adr/0016-operation-scoped-session-authority.md`, which does not exist in
  this repository. The proof binding — TLS exporter or client key — is undecided, so every TLS-side
  file above is a guess at an unmade decision.
- **Would collide with:** any unit touching the listener surface (`runtime.rs` `serve`/`serve_tcp`)
  or the shared `app/routes.rs` `router`; and any unit cutting a successor
  `contracts/substrate-wire/` version — that bump is a single serialising surface across the epic.

### Two corrections found while scoping

**1. A cited line is stale.** This story cites `crates/substrate-daemon/src/main.rs:63-64` as the
development-only TCP listener. Those two lines are `port,` and `})` — the tail of
`parse_egress_aperture`. The listener is `crates/substrate-daemon/src/runtime.rs:736`.

**2. Acceptance 5 is already false at HEAD.** This story says the development-only TCP transport
"must not gain this route". It has it: `serve_tcp` builds its service with the same `router(app)`
as the Unix path (`crates/substrate-daemon/src/runtime.rs:757` against `:508`), and that router
registers `/v1/pipe-sessions/{session_id}/attach` (`crates/substrate-daemon/src/app/routes.rs:76-77`).

Not an open exposure: `serve_tcp` bails unless both `--tcp-development-only` and
`--tcp-private-overlay` are set (`crates/substrate-daemon/src/runtime.rs:737-743`), so the route is
reachable only in an explicitly acknowledged development profile on a private overlay. But the story
asserts a property the code does not have, so either the acceptance must change or the router must
split — and nothing in the tree specifies a split. Decide it in the ADR this story still owes.
