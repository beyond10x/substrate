---
format: aep.planning-md/3
id: review-result:security-w20-u3-pass-1
kind: review-result
status: active
title: Security review, wave 2026-10-17b unit u3 (host-process daemon routes)
relations:
- reviews: story:host-process-lease
- reviews: story:host-process-cgroup-tree
revision: 1
---
# Security review, wave 2026-10-17b unit u3 (host-process daemon routes)

Reviewed: commit cf094499 against its base 8c1135ba. Verdict: needs change. Two portable cases
added (a strict supervisor fake that answers not-found for an id it does not hold); both red.

| # | where | finding | severity | origin |
|---|---|---|---|---|
| F1 | `crates/substrate-daemon/src/app/processes.rs` stop while starting | a stop or stop-by-owner accepted while a record is `starting` ends it on the driver's not-found; the start then dispatches and accepts the ended record as success, leaving a running program with no lease, deadline or stop | blocker | introduced |
| F2 | `processes.rs` restart re-attach | a failed re-attach is skipped with the pass reported complete, never retried, and a later lease expiry ends the record without evidence its cgroup emptied | warning | introduced |
| F3 | `processes.rs` stop | the stop operation is completed before the stop begins; a driver error other than not-found would leave the record `stopping` with no retry | note | introduced |

Held: the subject comes only from peer credentials; remote postures are refused on every route;
every store query is keyed by deployment and subject; refusals are durable before dispatch; the
environment is cleared; cwd is opened beneath with no symlinks; logs are owner-private, opened
without following links, read only after an exact record match; re-attach is by cgroup membership,
so a reused pid cannot be adopted. Not run: the delegated cases (whole-tree kill, double fork).
