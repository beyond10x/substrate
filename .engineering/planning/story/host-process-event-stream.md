---
format: aep.planning-md/3
id: story:host-process-event-stream
kind: story
status: active
title: Per-process event stream
relations:
- decomposes: epic:host-process-supervision
- informed_by: architecture-decision-record:host-processes-are-a-supervised-resource-family
- depends_on: story:host-process-start
- depends_on: story:host-process-logs
scope:
- confidence: inferred
  path: AGENTS.md
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: contracts/substrate-wire/0.24.0
- confidence: cited
  path: crates/b10x-substrate-sdk/src/lib.rs
- confidence: cited
  path: crates/b10x-substrate-sdk/src/transport.rs
- confidence: cited
  path: crates/substrate-daemon/src/app/processes.rs
- confidence: cited
  path: crates/substrate-daemon/src/app/routes.rs
- confidence: inferred
  path: crates/substrate-daemon/src/hosted.rs
- confidence: inferred
  path: crates/substrate-daemon/tests/processes.rs
- confidence: inferred
  path: crates/substrate-daemon/tests/runtime_vectors.rs
- confidence: inferred
  path: crates/substrate-host/src/host_process/log.rs
- confidence: inferred
  path: crates/substrate-store/src/events.rs
- confidence: inferred
  path: crates/substrate-store/src/host_processes.rs
- confidence: inferred
  path: crates/substrate-wire/src/generated/host_process.rs
- confidence: cited
  path: crates/substrate-wire/src/host_process_tests.rs
- confidence: cited
  path: crates/substrate-wire/src/lib.rs
- confidence: inferred
  path: scripts/gate.sh
- confidence: inferred
  path: spec/coverage.md
- confidence: cited
  path: spec/host-processes
- confidence: cited
  path: spec/host-processes-suite.json
- confidence: cited
  path: spec/host-processes/wire.yaml
- confidence: inferred
  path: xtask/bundle-source/0.24.0
- confidence: inferred
  path: xtask/src/bundle.rs
revision: 9
transitions:
- {from: "draft", to: "proposed", at: "2026-10-28T07:37:21Z", actor: "human:timo", revision: 5}
- {from: "proposed", to: "active", at: "2026-10-28T07:37:21Z", actor: "human:timo", revision: 6, executor: "agent:substrate-adr-0109"}
---
# Story: per-process event stream

## Context

Sessions wait on log lines and files in polling loops that time out or never match. A stream of a process's lifecycle and output lets a caller's watcher read what happened instead of polling; it ships before any such watcher.

Decided by `adr/0036-host-processes-are-a-supervised-resource-family.md` (S12). Specified in `spec/host-processes/`; the specification change lands first and its conformance suite drives the implementation.

## Scope

Derived 2026-10-23 by `story-scoper`. Each line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** `crates/substrate-daemon/src/app` (`GET /v1/processes/{process_id}/events`) — cited
- **Files:** `crates/substrate-wire/src/lib.rs` (`Event::validate_closed_shape` :2081, new bounds, `ADVERTISED_CONTRACT_BUNDLE` :18), `crates/substrate-daemon/src/app/processes.rs`, `app/routes.rs:112-120`, `crates/b10x-substrate-sdk/src/lib.rs:350`, `transport.rs:142`, `spec/host-processes/`, `spec/host-processes-suite.json`, successor bundle `0.23.0` — cited
- **Also likely:** `crates/substrate-store/src/host_processes.rs` (:1484 `append_event`), `crates/substrate-store/src/events.rs:203`, `crates/substrate-host/src/host_process/log.rs`, `crates/substrate-wire/src/generated/host_process.rs`, `runtime_vectors.rs`, `crates/substrate-daemon/tests/processes.rs`, `xtask/src/bundle.rs`, `scripts/gate.sh`, `AGENTS.md`, `CHANGELOG.md`, `spec/coverage.md` — inferred
- **Confidence:** medium
- **Open:** about 10 of the 12 acceptance scenarios do not exist yet; `wire.yaml:8` defines no stream item; bound values are unset.

## Acceptance

The following named conformance scenarios in `spec/host-processes/` pass, the delegated ones in the delegated lane and reported absent where the lane is absent:

- `host-process-lifecycle-appears-in-the-global-event-log`
- `host-process-output-lines-never-enter-the-global-event-log`
- `host-process-stream-interleaves-events-and-lines-in-order`
- `host-process-stream-ends-with-exactly-one-terminal-item`
- `host-process-stream-terminal-item-names-ended-by`
- `host-process-stream-resumes-from-its-cursor`
- `host-process-stream-reports-a-gap-for-discarded-log-bytes`
- `host-process-long-line-is-truncated-with-its-length`
- `host-process-non-utf8-line-is-marked-lossy`
- `host-process-slow-reader-never-blocks-the-program`
- `host-process-stream-on-an-ended-record-replays-and-closes`
- `host-process-stream-readers-bound-is-refused` (`process.events-readers-exceeded`)
