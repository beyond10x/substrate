---
format: aep.planning-md/1
id: epic:byte-plane-completion
kind: epic
status: draft
title: Byte-plane completion
summary: 'Phase 4''s remaining exit criteria whose design is already accepted: PTY sessions, sealed memfd secret slots, and network session transport with single-use proof-bound authority.'
owner: substrate
tags:
- phase-4
- wire
revision: 3
---
# Epic: Byte-plane completion

## Outcome

`ROADMAP.md` phase 4 reads *complete*: `README.md` § *Status* no longer lists PTY or network
session authority as absent, and design 05's condition for a live vendor harness — secret and
egress-adjacent capabilities — is met on the secret side.

## Why Now

`ROADMAP.md` orders the phases and forbids starting a later one while an earlier exit criterion is
open. Phase 4 is *in progress*: raw-pipe sessions, the distinct session resource, the read-only
execution capsule and the delegated model-free Agent lane are green
(`docs/plan/04-direct-byte-plane.md` § *Slices A–D*). The same plan's § *Later phase-4 slices*
names what remains, and three of those items already have accepted designs, so they can proceed
under invariant 8 with an ADR each and no new design document:

| capability | accepted design | fixed already |
|---|---|---|
| PTY session kind | `docs/design/05-streams-sessions-and-endpoints.md` § 2 | frames: input, output, resize, signal, exit, protocol-error; a PTY never substitutes for pipes; both kinds share the finite frame/byte/queue/idle bounds |
| sealed secret slots | `docs/design/06-authentication-secrets-and-trust.md` § *V1 decisions* 3 | sealed Linux `memfd` at a declared child descriptor; only the slot→descriptor mapping in the shaped environment; acquired after all admission checks; daemon copy closed after spawn; missing proof reports the capability absent |
| network session authority | design 05 § *V1 decisions* 3–4 | proof-bound, ≤ 60 s, single redemption, one concurrent attachment; reconnect needs a fresh authority; WebSocket over TLS; non-loopback listeners need TLS/mTLS or a trusted tunnel (`architecture/deployment-postures.md`) |

## Scope

Four stories, each an ADR, a successor bundle, host/daemon work and delegated-lane evidence — in
that order, which is the order invariant 8 requires:

1. `story:pty-sessions` — smallest, wholly local, exercises the successor-bundle path again.
2. `story:sealed-secret-slots` — the capability design 05 § *Progress* names as the reason a live
   vendor harness is refused.
3. `story:destination-bound-egress` — the other half of the vendor-harness pair; secrets without
   egress unlock nothing (added by the 2026-08-29 review against atlas).
4. `story:network-session-authority` — the last exit criterion, and the one with no consumer named.

Every story adds to a successor of `0.4.0`; no frozen byte changes (invariant 6). In sequence: PTY
is `0.5.0`, slots `0.6.0`, network authority `0.7.0`; two landing together share a bundle and the
ADRs say so. Each successor ships its renderer and checker and joins `scripts/gate.sh`. Each
capability is a fact the host verifies at startup; absent verification is a named refusal and the
capability document omits the fact (invariants 3, 4). Nothing here opens a second spawn path.

## Out of Scope

Reconnect (design 05 defines it as "a fresh authority", so it falls out of the network story), and
the hosted trust-envelope verifier (design 06 § 1, phase 7).

## Risks

- Three successor bundles in quick succession is a lot of frozen directories; batching two into one
  bundle trades that for a larger review. Decide per story when its predecessor lands.
- `memfd` sealing is Linux-only; the portable lane must prove the typed refusal, not skip the case.
- Channel binding for the network authority — TLS exporter (RFC 5705) or a client key — is an ADR
  decision; the design says only "proof-bound".

## Done When

Plan 04 § *Exit evidence* is met, `README.md` § *Status* no longer lists PTY or network session
authority as absent, and `ROADMAP.md` phase 4 reads *complete*.
