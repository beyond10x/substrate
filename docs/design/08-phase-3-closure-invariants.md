# Design 08: phase 3 closure invariants

**Status:** implemented and closed · **Date:** 2026-08-17

This document records the invariants found by the post-implementation adversarial review. Each has
deterministic crash, concurrency, capacity, and restart evidence under the
[archived closure disposition](../reviews/archived/2026-08-14-phase-3-closure-review-disposition.md).
It refines the earlier phase-3 plan without changing the immutable 0.1.0 bundle.

## Durable mutation boundary

- Once a bounded JSON mutation envelope exposes a valid caller operation id and raw `input`, every
  answered pre-dispatch refusal is durably bound to the canonical request. This includes typed
  schema errors, query errors, missing resources, invalid scalar limits, and non-ready lifecycle
  state. Malformed JSON, body overflow, and invalid or missing operation ids cannot be bound.
- Phase-3 binding hashes the canonical raw JSON `input` before typed defaults or reconstruction and
  a canonical query multimap. Valid form pairs are strictly decoded and sorted while duplicates are
  preserved; malformed percent/UTF-8 query data occupies a separate domain containing its exact raw
  bytes. Method and normalized address remain separate length-framed tuple fields.
- Failure to record a refusal is a store failure, never the unpersisted terminal refusal. A race
  with accepted work answers outcome-unknown. Exact replay and changed-input conflict remain
  authoritative.
- Existing replay/conflict lookup precedes explicit transactional per-subject and deployment-global
  row/byte quotas. A fresh id at capacity receives the one deliberately unbound, non-retriable
  `operation.ledger-capacity` refusal with no row, event, or dispatch. Operation ids are never
  silently forgotten, evicted, or reused within a deployment epoch. Internal safety work cites an
  existing caller operation; it does not mint a synthetic id or consume a hidden reserve.

## Two-stage resource dispatch

- Workspace creation first obtains a deterministic driver root identity without mutation. One
  transaction persists the accepted operation, root identity, and a non-callable provisional
  workspace. Only then may the driver create the root.
- Exec start first persists the accepted operation and a nonterminal exec membership, including
  workspace and lease authority. Only then may the driver spawn. Destroy, workspace expiry, and
  reconciliation treat accepted, running, and unknown membership as blocking until physical
  absence is positively proved.
- Driver create/start outcomes distinguish `not-dispatched`, `contained-absent`, and
  `outcome-unknown`. Post-mutation cleanup errors are never discarded or presented as
  not-dispatched. Unknown outcomes retain the durable provisional identity for observation and
  bounded reconciliation; restart never redispatches caller input.

## Workspace and exec state authority

- Every workspace-scoped host operation uses the same fixed, subject-scoped lock and atomically
  admits only a compatible durable lifecycle state. Reads and observations cannot race cleanup;
  an observation may not regress `destroying`, `expired`, or provisional `unknown` to `ready`.
- Lease claim atomically freezes the resource and projects `expiring` into readable state before
  background cleanup. Start, file mutation, and renewal refuse frozen state. Driver cleanup runs
  under the same workspace authority.
- A lease durably names its latest real authorizing caller operation. Creation installs the create
  operation; renewal atomically replaces it with the renewal operation. Claim, expiry, and cleanup
  failure events retain that cause and the initiating principal while naming the sweeper as the
  immediate actor; synthetic `system_*` operations do not exist.
- The first durably committed terminal exec observation wins across normal observation, signal,
  wait, recovery, and lease expiry. A store write returns the authoritative persisted observation,
  not unit success. Responses use that authority, and the driver is acknowledged only when the
  exact full terminal observation is durable. Expiry persists output and expiry state atomically;
  it preserves an earlier natural terminal outcome while updating the lease projection.

## Subject-scoped observation source

- The native source is `(deployment, source_scope, generation, seq)`. `source_scope` is an opaque,
  daemon-minted token durably bound to one authenticated `(deployment, subject)`; the raw subject
  need not appear on the wire.
- Sequence, retention, barrier, and generation are subject-local. A cursor is opaque and binds the
  source token, generation, and sequence. A cursor from another subject or epoch returns the same
  non-oracular reconciliation-required posture.
- Pull, push, and snapshots expose the source token consistently. Snapshot metadata includes an
  opaque `resume_cursor` for its inclusive barrier; consumers resume strictly after it and never
  construct cursors.
- No-cursor event pull is diagnostics, not durable bootstrap. A new durable consumer creates a
  complete barriered snapshot, applies it, then resumes from the returned cursor.

## Bounded reconciliation projection

- A snapshot is an authoritative complete current set for explicitly named resource kinds. Their
  absence is meaningful. Transactional per-subject limits reserve exactly 1,024 workspace items
  and 2,048 exec items; fresh over-cap mutations are durably refused before provisional resource
  insertion or driver dispatch.
- Native event provenance is one deterministic bounded window of at most 1,024 events strictly
  before the snapshot control barrier; the control event itself is not projected. The full
  idempotency ledger and deletion tombstones stay private. Metadata names the actual first/latest
  included sequence, count, and whether older retained history was truncated. Empty current state
  with no prior provenance is a valid zero-item snapshot even though its barrier event commits.
- The 4,096-item materialization budget is mechanically partitioned as 1,024 workspaces, 2,048
  execs, and 1,024 provenance events. Snapshot creation is a non-keyed control operation whose
  request body is exactly `{}` and whose bounded ephemeral metadata does not consume permanent
  operation-ledger capacity. Its barrier event uses a closed control cause, never a fake operation.
  Collectors query at most the remaining budget plus one and never scan or allocate an unbounded
  ledger.
- Terminal execs remain observable until a caller uses the keyed `exec.retire` mutation. Retirement
  is admissible only for terminal state, atomically removes the exec row, lease, and bounded output,
  preserves operation-ledger authority, returns typed absence, and emits `exec.retired`. This is the
  only phase-3 mechanism that frees exec capacity.
- Snapshot item types and identifiers are closed. Ordinals are contiguous, `item_count` is exact,
  completion requires returning the last item, and page cursors bind source and snapshot. The
  snapshot barrier event sequence must equal `through_seq` in the creation transaction; non-empty
  provenance history must end strictly before that barrier.

## Bounded maintenance and transport

- Request paths never await deployment-global maintenance. Scoped store admission checks lease
  deadlines transactionally and may coalesce a background nudge. Cleanup of one subject cannot
  head-of-line block another subject's ordinary requests.
- Lease cleanup, destroy continuation, and snapshot pruning claim fixed batches with durable fair
  order, next-attempt time, and capped backoff. Permanent failure cannot emit/log at the daemon tick
  rate or starve later work. Driver calls have deadlines.
- Event wakeup is subject-scoped and coalesces change hints. The journal read after a hint supplies
  the latest durable position; callback order is never mistaken for sequence authority. A bounded
  active-stream registry is registered before initial catch-up and removed by RAII. Noise from
  another subject cannot wake, lag, or close a stream.
- Raw Unix HTTP connections and WebSocket streams have fixed global and per-principal/subject
  limits, bounded idle/header/send lifetimes, and recover capacity on disconnect or timeout.
  Server-push WebSockets reject data frames and rate-bound control frames; inbound traffic never
  drives journal catch-up. The local HTTP transport disables keep-alive so an idle connection has
  no unbounded between-request state; its header deadline, maximum header count/buffer, total
  lifetime, and body read deadline are centrally configured. WebSocket frame/message/write bounds,
  send deadline, total lifetime, catch-up page budget, and global/subject permits are likewise one
  injectable policy. Permit and registry ownership is RAII on every preflight and timeout path.
- Every transaction that appends an event notifies its source scope after commit. Notification is
  an optimization over the durable journal and cannot be lost between subscription and catch-up.

## Required evidence

Tests must inject failures at every before/after-dispatch persistence edge, containment failure,
workspace observation race, terminal-state interleaving, maintenance retry/restart, stream scope,
cursor, snapshot boundary, and capacity boundary. Portable and delegated black-box lanes, both
contract bundles, exact manifest-selected vectors, format, clippy, links, ADRs, and offline schema
classification must all pass before the review disposition or phase status can say complete.
