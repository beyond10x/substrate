---
date: 2026-08-14
status: open-no-go
---

# Phase 3 post-curation closure review

This is the immutable findings input for the reopened phase-3 closure audit. The prior archived
minimum-host-slice review and its disposition remain historical evidence for the narrower surface
they assessed; they do not establish closure for the system after phase-3 lifecycle, stream,
snapshot, and lease additions.

## Verdict

**NO-GO.** Do not commit, push, archive this review, or mark phase 3 complete until every accepted
finding below has deterministic evidence and an independent rerun passes. Implementation work may
continue in the dirty worktree.

## A. Durable dispatch and resource lifecycle

1. Workspace creation and exec start require a two-stage protocol: declare deterministic physical
   identity, atomically persist accepted operation plus provisional membership, then dispatch.
2. Driver outcomes must distinguish not-dispatched, positively contained/absent, and unknown.
   Post-mutation cleanup failures may not be discarded or represented as not-dispatched.
3. Unknown/provisional exec membership blocks direct destroy, workspace lease cleanup, and restart
   cleanup until driver absence is positively proved. Periodic completion must be able to locate it.
4. A workspace root created before a driver/store error must remain durably discoverable by its
   exact predeclared root identity and undergo bounded observe-or-clean reconciliation.
5. Workspace lifecycle and all filesystem operations share subject-scoped lock/state admission.
   Stale GET observation cannot regress `destroying`, `expired`, or provisional `unknown` to ready.
6. Workspace expiry atomically freezes readable state before cleanup and cannot race exec/file
   admission. Cleanup runs under the same workspace authority.
7. Destroy failure remains destroying/pending, retries with bounded durable scheduling, and never
   reopens admission after partial deletion.

## B. Exec terminal and lease authority

8. The first durable terminal exec observation wins across normal completion, GET/output refresh,
   signal, restart, and lease expiry. Every write path enforces the same transactional rule.
9. Store writes return the authoritative full persisted observation/disposition. Responses use it;
   ACK occurs only for an exact terminal observation durably represented. Superseded driver
   terminal state is explicitly discarded without deleting the winner.
10. Exec lease expiry atomically persists full captured output and an expired terminal if no
    terminal won; if a natural terminal already won, preserve it and update only lease projection.
11. `wait_terminal` has deterministic lost-notification-race evidence. Accept/peer-error retry and
    socket cleanup RAII have injected early-error evidence.

## C. Durable refusal and finite ledger

12. After bounded JSON exposes a valid operation id plus raw input, every post-decode terminal
    refusal is ledgered: typed schema/query errors, missing resource, non-ready lifecycle, invalid
    base64/scalars/TTL/page values, and pre-dispatch limits. Exact replay and changed input conflict.
13. Raw typed-deserialization failure hashes the exact raw input; malformed JSON/body overflow or
    invalid/missing op remains unbound. Query fields are included in a documented canonical tuple.
14. Refusal persistence error returns store failure; an accepted race returns outcome-unknown. It
    may not return an unpersisted non-retriable refusal.
15. The exact replay ledger is finite: existing replay/conflict checks precede a hard per-subject
    quota; a fresh op at capacity gets the explicit unbound `operation.ledger-capacity` exception
    without event/dispatch. Internal safety uses existing authorizing operations.
16. Snapshot creation remains possible at op capacity as non-keyed bounded control POST with empty
    input; it does not create a permanent op or synthetic cause.

## D. Subject-scoped events and push transport

17. Native event source identity is `(deployment, opaque source_scope, generation, seq)`, with
    sequence, retention, barrier, and wakeup subject-local. Cross-scope/epoch cursors fail with one
    non-oracular reconciliation posture.
18. Pull, push, events, and snapshots expose consistent source scope. Snapshot metadata returns the
    exact opaque inclusive-barrier `resume_cursor`; consumers never construct cursors.
19. Wakeup is scope-specific, coalesces latest durable position, registers before initial catch-up,
    and is removed by RAII. Subject B flood cannot wake, lag, or close A.
20. Every event-appending transaction reports exact committed effects and notifies only after
    commit. Cover destroy terminal/conflict, observation-driven exec terminal, lease claim/failure,
    snapshot limit, and all mutation terminals.
21. WebSocket global/per-subject permits, bounded send deadline, catch-up cap, frame/control input
    bounds, protocol close for data frames, and permit/registry recovery are deterministic.
22. Raw Unix HTTP connections/tasks have fixed global and per-UID limits plus idle/header/lifetime
    bounds, and recover capacity after close/timeout. Accept and peer-credential errors continue.

## E. Bounded maintenance and snapshots

23. Request paths do not await a deployment-global sweep. Scoped transactional lease deadline
    admission freezes due state and nudges background maintenance; slow subject B cleanup cannot
    head-of-line block A.
24. Lease cleanup, destroy continuation, and snapshot pruning claim fixed fair batches with durable
    cursor/next-attempt/capped backoff and driver deadlines. Permanent first failure cannot starve
    later rows or emit/log every 250 ms; restart preserves schedule.
25. Snapshot is a complete quota-bounded current set for closed named resource kinds plus one
    bounded, honest provenance window. Metadata states covered counts/kinds and history bounds/
    truncation. Full op ledger is not scanned or projected.
26. The 4,096-item partition and workspace+exec quotas guarantee recovery liveness. Collectors use
    remaining+1 SQL limits, not unbounded Vec/N+1 scans. Terminal exec/output retirement is typed.
27. Snapshot items are closed discriminated values with coherent IDs. Ordinals are exactly
    contiguous, count exact, cursor scope/snapshot-bound, fabricated terminal/beyond cursors fail,
    and complete is true only after returning the final item.
28. Empty snapshots are valid with item_count zero. Snapshot barrier event seq equals through_seq;
    resume strictly after it. Rollback leaves no partial rows/event/control success.
29. No-cursor event pull is diagnostic only. Durable bootstrap is create snapshot, consume complete
    stable set, then resume its opaque cursor; noisy history eviction cannot lose current state.

## F. Contract and conformance blockers

30. Regenerate only development bundle 0.2.0; 0.1.0 bytes remain immutable. Add source scope,
    resume cursor, non-keyed snapshot control, quotas/history, coalesced wake, and typed causes.
31. Capability predicates are closed and truthful. Remove deferred `workspace.git` and
    `exec.network-aperture` predicates or add exact facts; checker cross-validates predicate fact,
    operator, value, and input compatibility.
32. Events and snapshot items are closed discriminated unions tying resource kind, transition,
    observation, resource, and identifier. Stream boundary kind/code pairs are closed.
33. JSON authority uses exact RFC 3339 date-time-with-timezone, strict bounded canonical base64,
    explicit numeric ceilings, exact RFC 6901, route/status/envelope relations, cursor/page/hash/
    content semantics, capability/lease/time relations, and lifecycle ordering invariants.
34. Vector grammar is clean-room reproducible and closed: typed setup/actions/driver outcomes,
    bounded string headers/raw repeats, meaningful fail invariants and numeric operands.
35. Packaging, hashing, origins/trust, and runner authorities are exact rather than prose-shaped;
    all schema targets resolve only under `schemas/`. Negative classifier tests cover non-schema
    `$schema` targets and every fixed authority.
36. Manifest-selected executable vectors cover every disputed semantic branch, including exact
    write limit, bootstrap, stream relations, quotas, crash windows, and changed-input conflicts.

## G. Required final evidence

37. Use failpoints/barriers and fake drivers/clocks/sinks/connection counters sufficient to cover
    provisional and terminal transaction edges, typed dispatch branches, terminal races, quota,
    bootstrap/snapshot, wake isolation, maintenance/backoff/restart, connection/WS bounds, and all
    durable-refusal categories.
38. Run format, workspace tests, clippy `-D warnings`, links, ADRs, both offline contract gates,
    negative JSON tests, exact 0.1 diff, reproducible 0.2 render, portable black-box lane, delegated
    systemd lane, and an independent read-only closure audit.
39. Correct STATUS/README/plan counts and claims only after bot-authored commit, private push, and
    remote synchronization. Add repository-local daemonloom-bot wrappers and preserve all repos
    private.

