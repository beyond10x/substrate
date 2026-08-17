# Design 03: lifecycle, observation, and reconciliation

**Status:** accepted v1 design · **Date:** 2026-08-13

Distributed execution is defined as much by missing answers as successful calls. Substrate makes
acceptance, observation, expiry, and recovery explicit so clients never guess whether work ran.

## 1. Command lifecycle

Every keyed resource mutation carries a caller-minted operation id and follows:

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

A declared bounded control operation can be non-keyed and therefore does not enter this lifecycle.
In 0.2 the only such mutation is reconciliation snapshot creation: it has `idempotency: none`, an
exact `{}` request body, and no operation-ledger row; its typed control event is the durable barrier.

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
4. **Events:** every authenticated `(deployment, subject)` owns an opaque daemon-minted
   `source_scope`, persisted generation, and monotonic sequence shared by pull and push. A
   generation changes only when continuity cannot be proved. Subject-scoped coalesced wakeups are
   hints over the durable journal; they carry no queued-history guarantee. Retention gaps use the
   barriered snapshot protocol accepted by architecture ADR 0017.
5. **Leases:** the minimum host slice remains valid without a lease. Phase 3 adds optional explicit
   workspace and exec TTLs, idempotent renewal, and a periodic cleanup sweeper. The durable lease
   record contains wall deadline, issuing wall time, Linux boot identity, issuing boot-relative
   time, and boot-relative deadline—never a process-local `Instant`. A boot change, monotonic
   regression, or wall/monotonic disagreement above 30 seconds expires conservatively. Workspace
   tombstones are retained no less than the native event window; cleanup failure remains an event
   and is retried.
6. **Bounded observation:** pull pages are capped at 1,000 events. WebSocket push uses the same event
   values and opaque cursor, caps durable catch-up at 16 pages, and applies explicit stream/send
   capacity rather than a fake queued-wakeup count. Its centrally configured admission policy also
   bounds global and subject streams, page/frame/message/write sizes, client control rate, send
   deadline, and total lifetime; data frames are protocol-closed. A cursor source/epoch mismatch or
   retention gap never silently fast-forwards.
7. **Stable reconciliation:** non-keyed snapshot control materializes a complete quota-bounded set
   of declared current resource kinds plus one honest bounded provenance window in the same SQLite
   transaction as its inclusive barrier event. Pagination reads only those rows and returns an
   opaque resume cursor. Missing rows produce `snapshot.incomplete`; later mutations cannot enter
   an existing snapshot. An empty current set is authoritative and valid.
8. **Destroy recovery:** workspace destroy commits a durable `destroying` observation before it
   touches the backing tree. Exec start refuses that state. Descriptor-relative cleanup advances in
   bounded batches without a total depth/item ceiling; after restart the daemon resumes it under the
   same workspace lock. Proof that the root is absent terminalizes the original accepted/unknown
   operation and its tombstone. The continuation is authorized by the persisted `destroying`
   resource plus the linked durable destroy operation and stored root identity; it neither
   redispatches arbitrary request input nor mints a fresh operation, and only an observed absent
   root permits success.
