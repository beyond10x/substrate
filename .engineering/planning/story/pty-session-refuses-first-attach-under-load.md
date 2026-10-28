---
format: aep.planning-md/3
id: story:pty-session-refuses-first-attach-under-load
kind: story
status: implemented
title: A fresh pty session refuses its first attach under host load
scope:
- confidence: cited
  path: crates/b10x-substrate-sdk/tests/unrecorded.rs
- confidence: cited
  path: crates/substrate-daemon/src/app/operations.rs
- confidence: cited
  path: crates/substrate-daemon/src/app/sessions.rs
- confidence: inferred
  path: crates/substrate-host/src/pty.rs
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-10-28T07:37:20Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-28T07:37:20Z", actor: "human:timo", revision: 3, executor: "agent:substrate-adr-0109"}
- {from: "active", to: "implemented", at: "2026-10-28T18:06:40Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"test_result":1,"review_outcome":2}}}
---
# Story: a fresh pty session refuses its first attach under host load

## Observation

On 2026-10-23 the delegated lane on `wave/2026-10-23-next` at `d8811be` failed once in
`crates/b10x-substrate-sdk/tests/unrecorded.rs:412` (`delegated_sdk_streaming_never_records_terminal_canaries`).
In the pty `success` ending's loop of 33 sequential sessions, a freshly started pty session
refused its first attach with `409 session.not-attachable`, "The pty session is not running under
an active lease." (`crates/substrate-daemon/src/app/sessions.rs:1239`). Four other sessions' builds
were running on the host at the time.

The same test passed in the delegated run at `a5ce055` (15:48 UTC+2) and five times out of five in
a delegated scope at `d8811be` immediately after (`probe-unrecorded.sh`). The raw-pipe loop in the
same failing run passed all 33.

## Not known

Which of the three conditions at `sessions.rs:1226-1228` failed (session state not `Ready`,
attachment not `Available`, lease not `Active`). The refusal does not say, and the test does not
record the session read back. Whether this predates wave 2026-10-23 is not known either.

## Acceptance

A reproduction under load (or a recorded session read back at the refusal) names the condition;
the fix makes a pty session returned by start attachable, or the SDK documents and waits for
readiness, with a delegated case that holds it.
