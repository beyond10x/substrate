---
format: aep.planning-md/3
id: story:live-output-stall
kind: story
status: implemented
title: Deliver bounded output bursts to active terminal consumers
refs:
- provider: github
  reference: https://github.com/beyond10x/substrate/issues/115
relations:
- implements: architecture-decision-record:live-output-stall
scope:
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: adr/0016-pipe-output-backpressure-is-terminal.md
- confidence: cited
  path: adr/0031-live-output-backpressure-has-a-stall-deadline.md
- confidence: cited
  path: adr/README.md
- confidence: cited
  path: crates/substrate-host/src/process.rs
revision: 8
transitions:
- {from: "draft", to: "proposed", at: "2026-10-02T22:00:42Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-02T22:00:42Z", actor: "human:timo", revision: 4}
- {from: "active", to: "implemented", at: "2026-10-02T22:33:10Z", actor: "human:timo", revision: 8, decided_on: {"recorded":{"test_result":3,"review_outcome":1,"verification":2}}}
---
## Problem
The current raw/PTY live queue cancels on instantaneous fullness even when an attached consumer drains normally. Mantle cannot reattach its Codex TUI before login; readiness alone remains red. See GitHub115 and accepted architecture-decision-record:live-output-stall; ADR0031 must be written and indexed before implementation.

## Acceptance

Named Rust scenarios at the real drain_capped/forward_frame seam: live_queue_burst_waits_for_consumer_without_losing_bytes; stalled_live_queue_reaches_deadline_without_falsifying_durable_truncation; closing_live_receiver_releases_waiting_drain_before_stall_deadline; unrecorded_drain_streams_accounts_and_never_accumulates_payload; live_drain_preserves_stream_and_bounded_capture; adversary_shared_stall_cannot_resume_sibling_stream. Larger-than-capacity bursts must preserve every byte and ordering. Deadline keeps the same refusal/cancellation path and pending sibling streams cannot resume forwarding afterward; disconnect, signal/timeout and accounting retain their guarantees. Full repository gate and relevant delegated execution lane pass. Mantle owns a separate combined real-transport proof and production runtime promotion before claiming the installed application fixed.

## Scope

crates/substrate-host/src/process.rs (implementation and existing colocated native tests); adr/0031-live-output-backpressure-has-a-stall-deadline.md; adr/README.md and ADR0016 supersession metadata as checker requires; CHANGELOG.md plus bounded transport documentation if necessary. Existing ESS projection explicitly excludes transport/process enforcement (spec/coverage.md); no new noun/projection or frozen contract bundle change is implied. Additional tests may live in current daemon/SDK delegated seam when needed.

## Authority and limits
Standing operator approval covers corrective waves. Independent managed tree based on current main2f81c434. Root owns AEP; agent implements; independent review required. Do not take over or reimplement the separately completed capture feature. No running service restart, workspace removal, release tag or consumer pin promotion is part of this unit's authorization.
