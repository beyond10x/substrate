---
format: aep.planning-md/3
id: review-result:security-w20-u3-pass-2
kind: review-result
status: active
title: Security review pass 2, wave 2026-10-17b unit u3 (host-process daemon routes)
relations:
- reviews: story:host-process-lease
revision: 1
---
# Security review pass 2, wave 2026-10-17b unit u3 (host-process daemon routes)

Reviewed: the fix commit eb1d9727 (pass 1's F1-F3). Verdict: needs change; F1-F3 hold.

| # | where | finding | severity | origin |
|---|---|---|---|---|
| P2-1 | `crates/substrate-daemon/src/app/processes.rs` undispatched start | a `starting` record whose start handler was dropped before dispatch is never held, so a lease-expiry stop leaves it `stopping` for the rest of the run | warning | introduced |
| P2-2 | `processes.rs` restart reconciliation | a record found `stopping` after a crash is ended, but its start operation stays `unknown` | warning | undecided |
| P2-3 | `processes.rs` reconcile pass | a stop landing between the pass's read and the abandon leaves the record `stopping`, unheld | note | undecided |

Fixed in e76a79f5: starts in flight are tracked with a drop guard; reconciliation concludes every
open start operation; an abandon re-reads the record. Red evidence: three cases failed before the
fix; daemon, host and store package tests green after it.
