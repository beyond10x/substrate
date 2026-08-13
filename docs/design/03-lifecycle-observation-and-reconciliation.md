# Design 03: lifecycle, observation, and reconciliation

**Status:** draft for review · **Date:** 2026-08-13

Distributed execution is defined as much by missing answers as successful calls. Substrate makes
acceptance, observation, expiry, and recovery explicit so clients never guess whether work ran.

## 1. Command lifecycle

Every mutation carries a caller-minted operation id and follows:

```text
unseen → admitted → accepted → terminal
              └──→ refused
```

`admitted` means request structure, authentication, scope, capability, and preconditions passed.
`accepted` means the operation may have changed driver state. Terminal records the typed outcome and
the latest provable resource observation.

The durable ledger distinguishes:

- never seen;
- accepted and in flight;
- terminal success;
- terminal answered failure.

Reusing an operation id with a different canonical request hash is a conflict.

## 2. Observed state

A successful mutation does not echo desired input. Service logic asks the driver to observe the
resource after acting and returns that observation with `observed_at`. Unknown fields remain `null`
with documented meaning. Desired state may be shown beside observed state for workloads, but the two
are never collapsed.

The daemon records the requested and applied sandbox separately. A response cannot infer
confinement from configuration alone.

## 3. Events

Each accepted state transition emits one typed event with monotonic sequence, resource identity,
operation id, actor label, transition, and observation time. Event pages and streams share the same
closed event vocabulary and retention window. Cursors outside retention fail explicitly.

Events support recovery and observability; they are not a workflow engine. Consumers own reactions,
retries, scheduling, notifications, and business history.

## 4. Leases and cancellation

Workspaces, execs, sessions, and workloads may carry leases. Renewal is an idempotent mutation.
Expiry produces a typed transition and best-effort cleanup appropriate to the resource; cleanup
failure remains observable.

Cancellation records intent, signals the driver, then observes the result. A cancellation response
cannot claim termination until the driver proves it. Repeated cancellation is safe under the same
operation id.

## 5. Unanswered outcomes

When no wire answer arrives, a client retains the original operation id and queries the ledger. It
must not mint a fresh id for an automatic mutation retry. An accepted operation without a provable
terminal result remains `unknown` until driver observation, ledger state, an event, or lease expiry
resolves it.

Clients may project the taxonomy into their own types. The canonical substrate classes remain
`refused`, `conflict`, `unserved`, `exhausted`, `failed`, `unreachable`, and `unknown`.

## Decisions required before implementation

1. Which ledger transitions must be durable before driver dispatch.
2. The ordering guarantee between terminal ledger state and emitted events.
3. Restart recovery rules for accepted operations whose driver observation is incomplete.
4. Lease clock tolerance and cleanup retention defaults.
