---
format: aep.planning-md/3
id: review-result:wave-2026-10-23-b2-security-pass-1
kind: review-result
status: active
title: Security review pass 1, unit b2 (stream caps and transport hand-over)
relations:
- reviews: story:transport-admission-and-stream-lifetime-disagree
- reviews: story:the-metrics-stream-cap-is-machine-readable
revision: 1
---
# Security review pass 1, unit b2 (stream caps and transport hand-over)

Agent type `aep:security-reviewer`. Branch `unit/2026-10-23-b2-stream-caps`, commit `32a012c`. Cases in module `app::tests::stream_caps_security_pass1` of `crates/substrate-daemon/src/app/tests.rs` (3 cases, all green). Verdict: 2 judgement findings, no red case.

```findings
- file: adr/0042-an-upgraded-connection-hands-its-transport-slot-to-its-stream.md
  category: judgement
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: with the per-source transport bound no longer counting upgraded sockets, one subject (or one shared source address) can hold a whole deployment-wide cap, all 32 session attachments or all 64 event/metrics streams with 16 subjects, and deny every other caller for the stream's hour.
- file: crates/substrate-daemon/src/app/metrics.rs
  line: 150
  category: contract-drift
  severity: note
  verdict: INFEASIBLE
  origin: pre-existing
  message: the metrics stream route upgrades without consulting metrics.stream, so on a host that publishes neither metrics.stream nor metrics.stream-bounds a stored terminal exec still gets a capped stream whose bounds the snapshot does not state.
```

Held: the hand-over runs only inside on_upgrade after the stream permit is taken and both permits live with the socket; a failed or aborted upgrade returns every slot (64 aborted handshakes over 16 subjects); 100 concurrent upgrades give exactly 64 x 101 and 36 x 429 and never count a socket in both budgets; per-family caps are separate; the Unix subject is still local:<uid> from peer credentials; published values and refusals equal enforcement; frozen bundles unchanged; SHA pin equals sha256 of 0.23.0 bundle.json.

Coordinator decision: fix finding 1 for session attachments with a per-subject attachment cap that keeps the bound one subject had before the hand-over, published in sessions.attachment-bounds (ADR 0042 amended first); the shared-source case for event and metrics streams is accepted and recorded in ADR 0042, since their per-subject caps hold. Finding 2: the metrics stream route refuses when the snapshot does not publish metrics.stream, with an already-published refusal; if none fits, record it in ADR 0042 instead of inventing one.
