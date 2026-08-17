# Disposition: phase 3 post-curation closure review

- **Review:** `2026-08-14-phase-3-closure-review.md`
- **Disposition date:** 2026-08-14
- **Verdict:** closed; findings 1–39 satisfied

## Result

The review was kept open while the implementation, contract, and evidence changed. A final
independent read-only audit found no remaining substantive or evidence blocker in findings 1–38.
Finding 39 closed only after the implementation was committed and synchronized to the private
`daemonloom/substrate` remote by `daemonloom-bot`; completion claims and this archival record were
then updated separately.

| Findings | Disposition | Principal evidence |
|---|---|---|
| 1–7 | Closed | Atomic provisional workspace/exec admission precedes dispatch; typed outcomes preserve unknown physical identity; shared lifecycle locks and durable destroying/expiry state prevent unsafe reuse. |
| 8–11 | Closed | First-terminal-wins store authority, exact driver acknowledgement, output-preserving expiry, lost-notification barriers, and connection-error continuation are deterministic tests. |
| 12–16 | Closed | Raw canonical request binding, durable refusal replay/conflict, injected refusal-store failures, ledger/resource ceilings, and capacity-safe empty snapshots execute against the production store and router. |
| 17–22 | Closed | Subject-local source identity, pull/push equivalence, greatest-position wake coalescing, exact post-commit effects, bounded raw connections, and four real TCP WebSocket adversaries prove scope and resource recovery. |
| 23–29 | Closed | Scoped request admission, fixed maintenance batches/deadlines, capped durable backoff, fair continuation across reopen, quota-complete snapshots, closed item types, and snapshot-first resume semantics are executable. |
| 30–36 | Closed | Immutable 0.1.0 remains byte-clean; reproducible 0.2.0 has closed capability predicates and unions, semantic schemas, exact origins/hashes, 19 manifest-selected runtime vectors, and 57 checked design vectors. |
| 37 | Closed | Failure injection covers dispatch/store edges, containment failure, terminal races, quotas, transport, maintenance batches/restart, and every durable-refusal category accepted by the review. |
| 38 | Closed | Format, workspace tests, strict Clippy, links, ADRs, both offline bundles, negative JSON tests, exact 0.1 diff, reproducible 0.2 render, portable 27-case runtime, delegated 38-case systemd runtime, and independent audit passed. |
| 39 | Closed | Repository-local bot wrappers are present; the remote is private; the implementation commit is bot-authored and synchronized before status and review archival are recorded. |

## Final contract facts

- `substrate-wire` 0.1.0: 114 classified JSON documents, 12 operations, 51 executable vectors,
  62 requirements, and 5 exact hash fixtures; bytes unchanged.
- `substrate-wire` 0.2.0: 165 classified JSON documents, 19 operations, 19 executable vectors,
  57 design vectors, 91 requirements, and 11 exact hash fixtures.
- Stable publication is deliberately not claimed. OCI packaging, signing, and downstream digest
  pinning remain later release work.
