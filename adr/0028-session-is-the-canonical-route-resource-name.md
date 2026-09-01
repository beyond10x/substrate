---
status: accepted
date: 2026-09-01
---

# ADR 0028: session is the canonical route and resource name

## Context

Substrate has one leased session resource with `pipes` and `pty` modes, but development contract
`0.14.0` serves its eight session operations beneath `/v1/pipe-sessions`. That mechanism-specific
path conflicts with the mode-neutral `session.*` operation ids, `ses_` durable ids and `session`
resource kind. The operator does not require backwards compatibility for this pre-1.0 API.

Because consumers may verify route bytes, Atlas ADR 0022 at
`atlas/architecture/adr/0022-substrate-session-route-is-a-breaking-development-migration.md`
authorises the coordinated breaking migration.

## Decision

Bundle `0.15.0` replaces exactly eight `/v1/pipe-sessions` route addresses with their
`/v1/sessions` equivalents. The daemon registers only the new family. There is no redirect,
compatibility route or public Rust alias.

The successor declares `breaking-development-v1`, names `0.14.0` as its predecessor, and records
the eight replacement pairs. Its checker proves 26 non-session addresses are preserved, eight old
addresses are removed, eight new addresses are added, and the paired operations retain their ids,
methods, scopes, idempotency, effects, exposure and request/result semantics. The exception is
closed to this exact migration; every other successor remains subject to the additive rule.

The daemon, SDK requests, MCP adapter, examples, clean-room inventory and public documentation
advance together and advertise the exact `0.15.0` bundle digest. Rust type names, schema authority
filenames, operation ids and the `pipes` mode are outside this route-only change.

## Consequences

Substrate's route describes the leased resource for both modes. Development clients must move
their paths and contract pin for release `0.5.0`; an old route receives the normal
`route.not-found` response. Every earlier bundle remains byte-identical and reproducible, but its
session routes are no longer served by the successor daemon.

The complete compatibility and verification design is
[design 16](../docs/design/16-sessions-are-not-pipe-sessions.md).
