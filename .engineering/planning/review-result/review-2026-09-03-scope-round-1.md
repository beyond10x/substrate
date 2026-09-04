---
format: aep.planning-md/1
id: review-result:review-2026-09-03-scope-round-1
kind: review-result
status: active
title: Scope critic (aep-planning:plan-critic-scope), round 1
tags:
- critic
- review
relations:
- reviews: epic:review-2026-09-03-findings
- reviews: story:line-patch-edits-apply-in-line-order
- reviews: story:directory-listing-survives-special-entries
- reviews: story:metrics-streams-are-bounded
- reviews: story:upgraded-connections-keep-their-permit
- reviews: story:lease-cleanup-reads-exec-state-only
- reviews: story:confined-processes-cannot-nest-user-namespaces
- reviews: story:seccomp-denies-af-vsock
- reviews: story:exec-oom-kills-the-whole-tree
- reviews: story:session-attachment-lifetime-is-an-accepted-decision
- reviews: story:hosted-admission-reuses-identity-authority
- reviews: story:daemon-image-serves-exec-or-says-it-cannot
- reviews: story:backend-recheck-hashes-only-on-change
- reviews: story:events-table-has-one-index
- reviews: story:unattached-claimed-session-is-contained
revision: 1
---
approve

**What I read:** the epic body (`aep artifact show epic:review-2026-09-03-findings`), the parent review register (`~/.cache/substrate-review/2026-09-03-substrate-in-depth-review.md`, all 14 findings), the full body of all 14 stories carrying `decomposes: epic:review-2026-09-03-findings` (`aep artifact show story:<id>` × 14, ids confirmed via `aep artifact graph --format json` filtered on that relation), and four unrelated existing "implemented" stories (`confinement-runtime-hardening`, `execution-resource-metrics`, `workspace-and-scratch-quotas`, `network-session-authority`, `hosted-trust-envelope-admission`, `production-tls-control-listener`) to rule out a finding already being claimed elsewhere in the graph. I extracted 18 promises from the epic — the 14 explicit finding→story mappings named in its Scope section, plus four cross-cutting constraints (closure-by-test-or-recorded-decision for every finding; the frozen-contract-bundle exclusion, which only story 2 can trigger; the named-refusal requirement for stories 6/7 under invariant 3; the observe-before-fix framing for story 4) — and traced all 18 to a story: 18/18.

Each of the 14 stories reproduces its finding's specifics accurately (file:line citations match, severity groupings match: 2 correctness, 3 capacity, 3 hardening, 3 decisions, 3 minor), and the three risk-driven constraints are honored verbatim in the relevant story bodies — story 2 and story 9 both name the successor-bundle path for a possible contract change, story 4's acceptance is phrased as observe-first ("the story records the observation ... and closes with no code change"), and stories 6/7 route to the named-refusal path. No story claims work outside the review's 14 findings, and no existing artifact in the graph already claims any of them.

**What I could not establish:** finding 8 in the review also notes `cpu.max` is clamped to one CPU as "a documented choice worth stating on the capability fact," but the review's own suggested-fixes list only cites `memory.oom.group=1` for this finding, and story 8's acceptance covers only the oom-group and refusal-reporting parts — whether the cpu.max documentation point was consciously dropped or simply not separately tracked, I could not tell from the story body alone; this reads as ambiguous rather than a clear narrowing, so it is not raised as a finding.

```findings
[]
```
