---
format: aep.planning-md/3
id: review-result:wave-2026-11-05-substrate-close
kind: review-result
status: active
title: 'Wave 2026-11-05 substrate close: out-of-scope rows decided'
relations:
- reviews: story:substrate-host-features-let-confinement-consumers-skip-git-and-kubernetes
- reviews: story:every-egress-reap-retries-through-eintr
revision: 1
---
# Wave 2026-11-05 substrate close: out-of-scope and note rows decided

The wave's U1 security pass, U1 adversary pass and U2 adversary pass left two rows outside a unit's
Acceptance. The wave lead decided each one at wave close. Every in-scope row was fixed in the wave:
the U1 rows by `cd22e190` (merged at `79243f9a`), the U2 rows by the adversary's cases `b7b08150`.
Counts: 1 accepted, 1 recorded without a move.

| # | from | row | decision |
|---|---|---|---|
| 1 | U1 adversary pass 1, `git_unserved.rs:64` | a build without `git` reconciles nothing, so a baseline and `.substrate-git-*` staging directories a Git build left behind stay on disk | accepted: reachable only when a build without `git`, configured with Git sources, opens a state root a Git build wrote; no consumer does that |
| 2 | U2 adversary pass 1, `substrate-executor/src/run.rs:350` | the executor waits with `waitpid(-1)`; not examined by the pass | recorded without a move: outside the story, which names the egress reaps only |

Row 1 is inferred from code and a search of the consumers in this repository; no external consumer
was surveyed.

```findings
[{"file":"crates/substrate-host/src/git_unserved.rs","line":64,"category":"judgement","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"scope out; accepted, no consumer opens a Git build's state root with a build without git"},
 {"file":"crates/substrate-executor/src/run.rs","line":350,"category":"judgement","severity":"note","verdict":"INFEASIBLE","origin":"pre-existing","message":"scope out; waitpid(-1) not examined, outside the egress reap story"}]
```
