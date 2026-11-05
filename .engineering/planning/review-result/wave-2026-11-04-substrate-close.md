---
format: aep.planning-md/3
id: review-result:wave-2026-11-04-substrate-close
kind: review-result
status: active
title: 'Wave 2026-11-04 substrate close: out-of-scope rows decided'
relations:
- reviews: story:abandoned-forwarder-reaped-through-eintr
revision: 1
---
# Wave 2026-11-04 substrate close: out-of-scope and note rows decided

The wave's three adversary passes, the lead's close read and the U4 spike left four rows that were
outside a unit's Acceptance or were notes. The wave lead decided each one at wave close.
Counts: 1 adopted as a story (2 rows), 1 accepted, 1 recorded without a move.

| # | from | row | decision |
|---|---|---|---|
| 1 | close read, `egress.rs:429`, pre-existing | `Drop for InstalledAperture` reaps its SIGKILLed forwarder with one un-retried `waitpid`; an `EINTR` leaves a zombie | adopted: `story:every-egress-reap-retries-through-eintr` |
| 2 | close read, `egress.rs:719`, pre-existing | `reaches_from_inside` reaps its probe child with one un-retried `waitpid`; on `EINTR` `status` stays `0`, which reads as reached | adopted: `story:every-egress-reap-retries-through-eintr` |
| 3 | U2 pass 1, `egress.rs:4771` | the new case skips without running when the user namespace cannot be opened | accepted: every `DownNamespace` case in the module follows this convention; whether the CI runner gets that far was not measured |
| 4 | U4 spike (kind with Calico, local images) | Calico enforces default-deny from a Pod's first instant, but always as a silent 3 s timeout; design 34 § 6.2 needs each attempt denied with EPERM or EACCES | recorded in the bodies of the two no-egress stories (`fb630678`), no status moved, as the approval ordered |

Rows 1 and 2 are inferred from code: whether any daemon thread installs a signal handler without
`SA_RESTART` was not measured.

```findings
[{"file":"crates/substrate-host/src/egress.rs","line":429,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"pre-existing","message":"scope out; adopted as story:every-egress-reap-retries-through-eintr"},
 {"file":"crates/substrate-host/src/egress.rs","line":719,"category":"judgement","severity":"warning","verdict":"CONFIRMED","origin":"pre-existing","message":"scope out; an interrupted reap reads as reached; adopted as story:every-egress-reap-retries-through-eintr"},
 {"file":"crates/substrate-host/src/egress.rs","line":4771,"category":"judgement","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"accepted as the module's DownNamespace skip convention"}]
```
