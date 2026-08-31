---
status: accepted
date: 2026-08-31
---

# ADR 0017: delegated context is verified before replay

## Context

[ADR 0011](0011-delegated-context-and-grant-attribution.md) requires delegated context in hosted
postures. The daemon currently inspects an existing operation and returns its replay before acting
on a verification refusal, allowing a missing or invalid context to read an earlier outcome.

## Decision

The current deployment's delegated-context requirement is enforced before any replay response.
Missing, malformed, invalid or expired required context is refused without altering the existing
operation row. A valid context may replay an operation originally recorded without attribution;
that historical row remains unattributed and is never rewritten. If the row records a grant, a
different verified grant remains `delegated-context.grant-conflict` and the same grant replays.

## Consequences

Replay is an authorized read of a durable outcome rather than a bypass around the current trust
envelope, while first-write attribution remains immutable.
