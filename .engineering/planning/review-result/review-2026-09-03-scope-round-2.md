---
format: aep.planning-md/1
id: review-result:review-2026-09-03-scope-round-2
kind: review-result
status: active
title: Scope critic (aep-planning:plan-critic-scope), round 2
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

**What I read:** the epic body (`aep artifact show epic:review-2026-09-03-findings`, now carrying `## Done When`), the parent review register (`~/.cache/substrate-review/2026-09-03-substrate-in-depth-review.md`, all 14 findings and the "smallest fixes" list), the full body of all 14 stories carrying `decomposes: epic:review-2026-09-03-findings` (`aep artifact show story:<id>` × 14), the round-1 scope verdict (`review-result:review-2026-09-03-scope-round-1`, approved, 0 findings), `aep artifact graph` (confirms exactly 14 `decomposes` edges into the epic, no other artifact claims any of these findings), and `aep artifact validate` (reports `valid`; the two items it flags — 10 unscoped unrelated stories and round-1's prose-only findings block — are pre-existing and outside this set). I extracted 18 promises from the epic (the 14 explicit finding→story mappings plus 4 cross-cutting constraints: closure-by-test-or-recorded-decision now stated explicitly as `## Done When`; the frozen-contract-bundle exclusion story 2 may trigger; the named-refusal requirement for stories 6/7 under invariant 3; the observe-before-fix framing for story 4) and traced all 18 to a story: 18/18.

Checked each of the 7 narrowed stories (`backend-recheck-hashes-only-on-change`, `confined-processes-cannot-nest-user-namespaces`, `daemon-image-serves-exec-or-says-it-cannot`, `exec-oom-kills-the-whole-tree`, `hosted-admission-reuses-identity-authority`, `metrics-streams-are-bounded`, `session-attachment-lifetime-is-an-accepted-decision`) against the finding-specific content moved into `## Notes`: in every case the moved clause is stated as still delivered ("delivered in this story and checked by its own probe/case", "not this story's acceptance" because it is "already covered by" an existing test, or "the statement holds either way") rather than dropped — none reads as a silent narrowing. Invariant 3's named-refusal requirement for stories 6/7 survives in Notes for `confined-processes-cannot-nest-user-namespaces` and was already in `seccomp-denies-af-vsock`'s single-statement acceptance (unchanged). No story's `## Notes` claims work the epic's Scope or Out of Scope did not ask for.

**What I could not establish:** `exec-oom-kills-the-whole-tree`'s Context still carries finding 8's third clause — "`cpu.max` is clamped to one CPU … which is a documented choice worth stating on the capability fact" — and neither its Acceptance nor its Notes commits to that documentation step; this is the same ambiguity round 1 flagged and declined to raise as a finding, and it is unchanged by this round's edits, so I am carrying it forward as unresolved rather than as a new finding (`.engineering/planning/story/exec-oom-kills-the-whole-tree.md`, `## Context`).

```findings
[]
```
