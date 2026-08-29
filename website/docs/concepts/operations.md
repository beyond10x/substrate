---
title: Operations and observations
description: How durable operation identity, typed outcomes, events, and leases make execution recoverable.
---

# An effect begins with a durable operation

Every resource mutation carries a caller-minted operation ID. Substrate records the provisional
operation before dispatching to a driver.

```text
request
  → validate
  → reserve operation durably
  → verify capability snapshot
  → dispatch bounded action
  → re-read observed state
  → commit outcome and event
  → answer
```

The order closes a common failure gap: if the connection breaks after acceptance, the caller can ask
about the same operation rather than creating a second effect.

## Retry identity

Reusing the same operation ID with the same body returns the same logical outcome. Reusing it with a
different body is a conflict.

Do not mint a new ID merely because a response was lost. Query the operation ledger first.

## Observed answers

A successful mutation returns the resource as observed after the action. It does not echo the
request and call that success.

- A non-zero process exit is an observed exec result, not a failed API operation.
- Truncated output remains an explicit observation.
- Missing facts remain absent; they do not become zero.
- Restart can turn an in-flight outcome into `unknown` when the terminal effect cannot be proved.

## Answered outcome classes

| Class | Meaning | Typical next move |
|---|---|---|
| `refused` | a guard, authority, validation, or precondition said no | change the request or authority |
| `conflict` | current state disagrees with the mutation | re-read and decide again |
| `unserved` | this deployment does not implement the requested operation | choose another deployment or stop |
| `exhausted` | the request is valid but capacity is unavailable | free capacity or retry the same operation |
| `failed` | an admitted operation ended in a driver failure | repair, then reconcile the same operation |

Transport silence is different: no answer means acceptance is unknown. The ledger, replayable events,
and reconciliation snapshots exist for that case.

## Leases turn disappearance into state

Workspaces, execs, and sessions may carry a renewable lease. Expiry is a typed transition with an
event and a reason. A vanished client therefore leaves an observable lifecycle outcome rather than
an indefinitely assumed owner.

See [the contract surface](../reference/contract.md) for resource routes and
[deployment postures](../guides/deployment.md) for the trust boundary around them.
