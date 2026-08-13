# Design 03: lifecycle, observation, and reconciliation

**Status:** accepted v1 design · **Date:** 2026-08-13

Distributed execution is defined as much by missing answers as successful calls. Substrate makes
acceptance, observation, expiry, and recovery explicit so clients never guess whether work ran.

## 1. Command lifecycle

Every mutation carries a caller-minted operation id and follows:

```text
unseen → admitted → accepted → terminal
              │          └──→ unknown → terminal
              └──→ refused
```

`admitted` is a transient service state: request structure, authentication, scope, capability, and
preconditions passed, but no durable driver authority exists yet. `accepted` means the durable
before-dispatch transaction committed and the operation may change driver state. `unknown` means
acceptance is durable but a terminal observation cannot currently be proved. Terminal records the
typed outcome and the latest provable resource observation.

The durable ledger contains one of:

- `refused` — admission answered no before dispatch;
- `accepted` — dispatch may be in flight;
- `unknown` — accepted, but the current outcome is unproven;
- `terminal` — success or a typed answered failure plus the latest provable observation.

Absence of a row is the subject-scoped not-found answer; neither `unseen` nor `admitted` is a durable
ledger state.

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

Beginning in phase 3, workspaces, execs, sessions, and workloads may carry explicit leases. Renewal
is an idempotent mutation.
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

## V1 durability and recovery decisions

1. **Before dispatch:** one transaction reserves `(deployment, subject, op)`, stores the canonical
   request hash, admitted capability/config snapshot, and `accepted` state. The transaction is
   durably committed before the driver may mutate anything. Refusal before acceptance is also
   durable under the operation id so replay returns the same answered outcome.
2. **After dispatch:** resource/operation observation, terminal ledger state, and its event sequence
   are committed in one transaction before the terminal answer is sent. A crash after driver
   mutation but before that transaction leaves `accepted`, never a fabricated success.
3. **Restart:** every accepted/nonterminal operation is reconciled against the selected driver's
   observed state. Provable state becomes terminal; absence that is itself authoritative may become
   failed/refused as defined by the operation; ambiguity remains `unknown`. The daemon never repeats
   a mutation merely because it restarted.
4. **Events:** one persisted generation and monotonic sequence are shared by pull and push. A
   generation changes only when continuity cannot be proved. Retention gaps use the barriered
   snapshot protocol accepted by architecture ADR 0017.
5. **Leases:** not served in the minimum host slice. Phase 3 must add monotonic-clock accounting,
   30-second maximum clock tolerance, explicit per-resource TTLs (no implicit lease), and a
   deployment-configured cleanup/tombstone retention no shorter than event retention before the
   capability appears.
