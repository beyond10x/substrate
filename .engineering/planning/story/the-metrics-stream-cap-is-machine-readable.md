---
format: aep.planning-md/1
id: story:the-metrics-stream-cap-is-machine-readable
kind: story
status: draft
title: The metrics stream cap is machine-readable
scope:
- confidence: cited
  path: crates/substrate-daemon/src/app/events.rs
- confidence: cited
  path: crates/substrate-daemon/src/app/metrics.rs
revision: 3
---
# Story: The metrics stream cap is machine-readable

## Context

`story:metrics-streams-are-bounded` added a client-visible `429` with the code
`metrics.stream-capacity` to `GET /v1/metrics/stream`. The cap of 4 per subject and 64 per
deployment, and the code itself, exist in daemon source and — since that story — in
`website/docs/guides/storage-and-metrics.md`. **A client cannot read either from anything the wire
publishes.**

No bundle byte was owed and none was written: `contracts/substrate-wire/0.15.0/refusals.json` is
`b10x.substrate-session-refusals.v1`, 36 rows, every prefix `session`. The sibling cap
`event.stream-capacity` is in the same position, so this is the second instance of one gap rather
than a new one. Confirmed twice during the 2026-09-04 security wave
(`review-result:adversary-u1-pass-1`, `-pass-2`), both times agreeing that prose is the only
publication available today.

## Acceptance

A client reading only what the wire publishes can discover the metrics stream's per-subject and
global caps and the refusal code that names them.

## Notes

This is a **successor bundle plus an ADR** under invariant 8, not a line: it needs a decision about
where non-session refusals are published, which reaches `event.stream-capacity` too. Invariant 6
forbids editing a released bundle, so `0.16.0` is the earliest home.

Worth deciding at the same time whether stream capacity belongs in `refusals.json` or in the
capability facts a client already reads before it opens a stream.
