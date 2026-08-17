---
status: accepted
date: 2026-08-14
---

# ADR 0007: protocol processes use raw-pipe sessions

## Context

Substrate phase 3 can execute and later return bounded output, while phase 4 already owns leased
interactive sessions. Agent harnesses such as a JSONL or JSON-RPC app server require live,
bidirectional stdin/stdout with stderr kept separate. A PTY changes byte and terminal semantics and
cannot safely carry these protocols. Direct host launch would bypass Substrate's process-tree,
environment, resource, network, lease, and terminal-observation guarantees.

[Architecture ADR 0023](../../../architecture/adr/0023-governed-harness-execution-is-defense-in-depth.md)
requires the first integration to be model-free, no-egress, and confined through a Substrate-owned
session.

## Decision

Phase 4 supports two explicit session modes: `pty` for human terminal interaction and `pipes` for
machine protocols. Both create a leased session resource before attachment and use the same
operation-scoped authority, ownership, capability, cancellation, and terminal-observation rules.

A pipe channel preserves three distinct streams. Its closed frame vocabulary covers stdin bytes,
stdout bytes, stderr bytes, close-stdin, signal/cancel, exit, truncation, and protocol error. Input
and output frames are individually and cumulatively bounded; backpressure has finite queue and time
limits. Only Substrate's observed exit after whole-cgroup cleanup is terminal process evidence.

The first implementation serves an owner-permissioned Unix socket, one attachment, one no-egress
Linux host session, and synthetic protocol processes. It includes no PTY, network transport,
credential secret slot, public egress, model call, or agent-specific field. Missing confinement
refuses session creation without a direct-host fallback.

## Consequences

- Machine protocols are never transported through a terminal emulator.
- Agent can implement its own execution port over released Substrate bytes without a source
  dependency; Substrate remains unaware of Codex, Claude, prompts, tools, and approvals.
- Live model-using harnesses remain blocked until named sealed secret delivery and destination-bound
  egress are separately implemented and proven.
- Phase-4 contract vectors must cover fragmentation, ordering, half-close, queue pressure, client
  loss, lease expiry, cancellation, child descendants, and terminal replay/reconciliation.
