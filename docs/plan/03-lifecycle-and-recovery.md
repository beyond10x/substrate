# Plan 03: lifecycle and recovery

**Status:** closure implementation in progress · **Date:** 2026-08-13

The post-implementation adversarial review reopened this phase. The additional closure authority is
[Design 08](../design/08-phase-3-closure-invariants.md); the exit criteria below remain unmet until
that design has deterministic evidence and an independent audit passes.

Phase 3 extends the completed minimum host slice without rewriting the immutable
`substrate-wire` 0.1.0 bytes. Its authority is the `0.2.0` development bundle: it preserves the
twelve routes, adds seven routes, and explicitly clarifies four unsafe 0.1 expectations—an accepted
terminal machinery failure is not advertised as retryable, and a restart reconciles an accepted
but unanswered operation to `unknown` rather than leaving it `accepted`; the corrected transport
fixture also expresses the 2 MiB envelope required to carry an exactly 1 MiB decoded file, and a
durably terminal write-limit refusal is not advertised as retryable under the same operation id.

## Contract surface

The 0.2.0 registry contains the existing twelve operations plus exactly seven lifecycle operations:

| Operation | Method and path | Bound |
|---|---|---|
| `event.list` | `GET /v1/events` | at most 1,000 retained events per page |
| `event.stream` | `GET /v1/events/stream` (WebSocket upgrade) | bounded catch-up and coalesced subject-local wakeup |
| `reconciliation.snapshot.create` | `POST /v1/reconciliation-snapshots` | non-keyed exact-`{}` control creation of one bounded subject view; no permanent operation row |
| `reconciliation.snapshot.get` | `GET /v1/reconciliation-snapshots/{snapshot_id}` | at most 1,000 stable items per page |
| `workspace.lease.renew` | `POST /v1/workspaces/{workspace_id}/lease/renew` | explicit bounded TTL |
| `exec.lease.renew` | `POST /v1/execs/{exec_id}/lease/renew` | explicit bounded TTL |
| `exec.retire` | `DELETE /v1/execs/{exec_id}` | keyed terminal-only removal of durable exec state and bounded output |

Workspace creation and exec start accept an optional explicit `lease_ttl_ms`. Omission means no
lease; there is no implicit lease. Phase 3 adds no Git serving, session/stdin/PTY byte plane,
Docker, workload, cloud, or connector runtime behavior.

## Transaction and recovery invariants

1. Each authenticated `(deployment, subject)` has an opaque `source_scope`, durable generation, and
   monotonic unsigned sequence. Ordinary restart preserves them. Reset, restore, destructive
   repair, or missing sequence proof requires a fresh generation. Cursors bind the opaque scope,
   generation, and sequence without exposing the raw subject.
2. Operation acceptance/refusal, terminal resource and operation observations, tombstones, lease
   transitions, and their event sequence are committed in the same SQLite transaction. An event
   never precedes readable state.
3. Pull and WebSocket push emit the same `(deployment, source_scope, generation, seq)` identities.
   Push is only a bounded notification/catch-up transport over the journal. Subject-local wakeups
   coalesce change hints; every wake reads the latest durable position rather than treating callback
   order as sequence authority. Send pressure and catch-up bounds close with the last delivered
   cursor, and pull is the recovery path. A stream holds one of 64 daemon-wide and four
   subject-local permits, accepts at most 1 KiB control frames, rejects data frames, rate-limits
   control frames to 120/minute, applies a five-second send deadline, and ends after one hour. A
   catch-up performs at most sixteen 64-item durable reads before returning the client to pull.
   It is not the phase-4 process byte plane.
4. An outside-retention cursor, another source scope, and a different generation all return the
   same non-oracular reconciliation-required posture. None silently resumes at the oldest retained
   event or reveals which source/epoch component mismatched.
5. Reconciliation snapshots are fully materialized at one inclusive `through_seq` barrier and
   return the exact opaque `resume_cursor`. Pages read the materialization, not live tables or a
   live `OFFSET` view. They include all current workspaces (at most 1,024), all current execs (at
   most 2,048), and at most 1,024 actual native events strictly before the control barrier; the full
   idempotency ledger and deletion tombstones are not projected. Terminal execs remain current until
   explicit keyed `exec.retire` atomically removes their row, lease, and bounded output while
   preserving operation authority and emitting typed absence provenance.
   Missing materialized rows make the snapshot explicitly incomplete and forbid advancement.
   Expired metadata and its cascade-owned items are physically collected. Active snapshots are
   bounded to 64 and materialization to 4,096 items per subject; 1,024 bounded expiry markers keep
   `snapshot.expired` distinct from a never-created identifier.
6. Restart never redispatches a caller mutation. Accepted operations and nonterminal exec
   observations reconcile to provable terminal state or `unknown`, with a journal event in the same
   transaction.
7. A terminal exec remains in driver memory until its full observation and bounded output have
   committed durably. Maintenance acknowledges it only after that commit. Durable `exited`,
   `cancelled`, and `expired` states cannot regress under concurrent observation or sweeping.
8. Filesystem blocking work uses a bounded 16-slot blocking pool. SQLite calls use a separately
   bounded 16-slot blocking lane whose permit is acquired asynchronously, so saturation applies
   backpressure without starving accept, event, or process tasks.
9. The local Unix transport admits at most 128 live connections and 32 per authorized UID. Header
   reads expire after five seconds, header count is capped at 64, the HTTP buffer is capped at
   64 KiB, keep-alive is disabled, and a connection is cancelled after five minutes. Request bodies
   are capped at 2 MiB and must complete within ten seconds. Accept and peer-credential failures are
   refused locally and the accept loop continues; connection and stream permits are RAII-owned.
10. Every workspace host operation and cleanup serializes through a bounded per-subject lock domain
   with fixed workspace stripes; unrelated subjects never share a mutex. A destroy commits
   `destroying` before filesystem cleanup, which blocks new host or exec admission. Cleanup
   uses descriptor-relative 4,096-item batches with monotonic progress, has no path-depth or total
   item cap, and is idempotently resumed after restart. An absent backing root terminalizes the
   original accepted/unknown destroy operation and tombstone as success. This is continuation of
   the persisted `destroying` lifecycle against its stored root identity, not redispatch of the
   caller mutation: no new operation is minted, and success requires a driver absence observation.

## Lease clock and cleanup invariants

- A lease persists its wall deadline, issuing wall time, Linux boot identity, issuing boot-relative
  monotonic reading, and boot-relative monotonic deadline. A process-local `Instant` is never
  persisted.
- Within the same boot, monotonic elapsed time governs. Wall/monotonic disagreement above the fixed
  30-second tolerance makes continuity unprovable and expires conservatively. A changed or missing
  boot identity also expires conservatively.
- Renewal is keyed and idempotent, cannot revive an expired lease, and replaces the TTL from the
  observed renewal instant. The lease also atomically replaces its stored authorizing operation
  with that real renewal operation; background events never invent a synthetic operation id.
- Expiry is claimed durably, then cleanup is best effort and idempotent: exec process trees are
  killed before workspace collection. Completion records an expired observation,
  cleanup result, tombstone when removed, and event. Failure remains observable and retryable by
  the daemon; it is never reported as successful deletion.
- The full operation ledger uses a finite explicit per-subject quota and remains authoritative for
  exact replay/conflict. It also has a deployment-global row/byte bound, and existing lookup occurs
  before quota checks. A fresh id at capacity receives the explicit unbound, non-retriable
  `operation.ledger-capacity` exception without a row, event, or dispatch; cleanup authorized by
  existing operations remains usable. Snapshot provenance is independently bounded and states its
  truncation boundary.

## Acceptance evidence

- Contract vectors cover duplicate/replay, restart without redispatch, retention gap, generation
  reset, pull/push identity, bounded stream backpressure, stable paginated reconciliation,
  resource-capacity refusal, terminal exec retirement, incomplete snapshots, lease renewal, expiry,
  clock discontinuity, and cleanup failure.
- Both immutable bundle gates classify every JSON authority and meta-validate every declared Draft
  2020-12 schema offline. Negative tests prove unclassified JSON, invalid bootstrap authorities,
  invalid declared payloads, and invalid schemas all fail closed.
- Store and HTTP tests force concurrent mutations across snapshot pages and prove a stable barrier.
- Each fresh portable black-box run reports its executed inventory and must prove startup,
  dual-daemon refusal, and the portable HTTP behaviors.
- Each fresh delegated host run reports its executed inventory and must additionally prove capacity
  pressure, trapped-signal classification, whole-cgroup cleanup, and leased exec expiry.
- Manifest-selected runtime tests execute the disputed vectors at their declared bundle versions.
  Path depth, subject hiding, and TERM remain exact 0.1 behavior; machinery retryability, restart
  reconciliation, and body-limit corrections execute exactly from their explicit 0.2 forms.

## Exit

Phase 3 is complete only when the 0.2.0 bundle is reproducible, all nineteen routes are implemented,
portable and delegated lanes pass, restart and adversarial store tests pass, documentation matches
the implementation, and the private bot-authored `main` is clean and synchronized.
