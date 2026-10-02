---
status: accepted
date: 2026-10-02
---

# ADR 0031: live output backpressure has a stall deadline

Supersedes [ADR 0016](0016-pipe-output-backpressure-is-terminal.md).

## Context

An interactive process can produce more than one bounded queue of terminal output in a burst.
Immediate failure on a full queue treats scheduler timing as a stalled consumer, even when an
attached client continuously drains. Mantle exposed this with a Codex terminal redraw (issue #115).
An indefinitely awaited send would restore the cleanup deadlock ADR 0016 prevents.

## Decision

The host awaits a live output slot for at most one second per frame. The deadline begins when
reservation is attempted and is not extended while that frame waits. A successful reservation
precedes payload copying and preserves queue occupancy accounting, including reserved positions.
The existing frame, queue, streamed-output and recorded-capture byte ceilings remain unchanged.
The reader retains only its existing fixed read buffer while waiting; there is no overflow queue.

When the deadline expires, the host records the existing `session.output-backpressure` refusal,
stops live forwarding and requests normal whole-tree cancellation. It continues draining the
child's pipes, retaining only the independently admitted capture bytes. Closing the receiver wakes
any pending reservation immediately. Existing signal, timeout and resource-bound paths close that
receiver before terminal reconciliation. Unrecorded capture retains its cancellation-on-close and
zero-payload-retention behavior and its exact output and queue counters.

Already queued output remains ordered. Backpressure refusal does not manufacture durable output
truncation: truncation continues to mean that the admitted byte ceiling was crossed. No payload
is dropped to keep a session alive. An unattached or stalled client can still terminate its session,
but a temporary full queue alone is not terminal.

## Compatibility and verification

No wire type, route, capability, refusal vocabulary or persisted format changes. Frozen contract
bundles remain untouched; this is host scheduling beneath their existing bounded-queue contract.
This decision introduces no new entity. The current ESS accepted-operation projection excludes
session transport enforcement; its existing coverage remains accurately bounded.

Regressions exercise the actual host drain with a burst exceeding all queue positions, asserting
exact delivery to an active consumer and no cancellation; a stalled reader, asserting the fixed
wait and named terminal refusal; receiver closure, asserting prompt drain completion in recorded
and unrecorded modes; and unchanged capture/streamed-output ceilings and queue accounting.
The portable gate and applicable delegated lifecycle tests remain required.
