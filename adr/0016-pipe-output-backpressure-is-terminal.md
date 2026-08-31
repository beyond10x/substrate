---
status: accepted
date: 2026-08-31
---

# ADR 0016: pipe output backpressure is terminal

## Context

A raw-pipe session creates its bounded live-output queue before attachment. The drain task awaits a
queue send, so an unattached process can fill the queue and prevent output draining, terminal
observation and whole-tree cleanup. Reusing the durable output-truncation bit would be false: that
bit says the admitted capture-byte ceiling was crossed, not that a live consumer failed to drain.

## Decision

The live queue reserves one control position. Output enqueue is non-blocking. When all declared
output positions are occupied, one atomic winner records `exhausted/session.output-backpressure`,
stops live forwarding, and requests normal whole-tree cancellation. The drain continues so the
child cannot block on its pipes. The terminal exec is `cancelled` with that refusal; durable stdout
and stderr retain their existing independent byte ceilings and truncation meaning.

An attachment receives already queued output, a `protocol-error` naming the backpressure refusal,
and the terminal exit. Lease-clock absence follows the same durable pre-dispatch refusal path as
workspace and exec starts.

## Consequences

Queue capacity remains a real bound and no output is silently misclassified. A client that does not
attach or drain may cause its session to end, but it can always distinguish that outcome from
ordinary output truncation.
