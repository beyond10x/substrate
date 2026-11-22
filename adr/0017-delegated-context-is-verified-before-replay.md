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

## Addendum — 2026-11-23: requests that arrive at the same time

This addendum changes no decision above. It states the decision for the case the text does not
name: two requests of one subject and one operation identity that are in flight together.

The grant is compared in the transaction that finds the row, for every request that finds it. A
request that loses the write of a row to a concurrent one is answered as a retry of that row:
the stored outcome when its verified grant is the row's, `delegated-context.grant-conflict` when
it verified another grant. A refusal decided from a request's own envelope or delegated context
is never replaced by a row's stored outcome.

The grant is compared before the request hash, so a request under another grant is answered
`delegated-context.grant-conflict` also when its input differs.

Unchanged and recorded here because the text above leaves it open: where the deployment does
not require delegated context, a valid request without one replays a row that records a grant.
A deployment that requires context refuses that request `delegated-context.absent` first.
