---
format: aep.planning-md/1
id: review-result:review-2026-09-03-parallel-safety-round-1
kind: review-result
status: active
title: Parallel-safety critic (aep-planning:plan-critic-parallel-safety), round 1
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
Now I have exact citations. Let me finalize the verdict.

**needs-revision**

story:backend-recheck-hashes-only-on-change — this story and four siblings (story:confined-processes-cannot-nest-user-namespaces, story:daemon-image-serves-exec-or-says-it-cannot, story:exec-oom-kills-the-whole-tree, story:seccomp-denies-af-vsock) all declare `crates/substrate-host/src/probe.rs` as cited scope, three of them also share `crates/substrate-host/src/process.rs`, and none of the five bodies or relations names another — `aep artifact waves` (10 of its 15 reported collisions are this cluster); .engineering/planning/story/backend-recheck-hashes-only-on-change.md:14,16

story:line-patch-edits-apply-in-line-order — this story and story:directory-listing-survives-special-entries both cite `crates/substrate-host/src/fs.rs` as scope and neither body mentions the other — .engineering/planning/story/line-patch-edits-apply-in-line-order.md:14

story:metrics-streams-are-bounded — this story and story:lease-cleanup-reads-exec-state-only both cite `crates/substrate-daemon/src/app/service.rs` as scope and neither body mentions the other — .engineering/planning/story/metrics-streams-are-bounded.md:16

What I read: all 14 stories under `decomposes: epic:review-2026-09-03-findings` (`aep artifact list --format json`), each read with `aep artifact show <id>`, cross-checked against raw frontmatter (`grep -n path:` on each story file), and against `aep artifact waves`, which independently derives the same 15 file-level collisions across 12 pairs and assigns 5 waves to keep them apart. Surface established: 14 cited, 0 inferred, 0 unplaceable — every one of the 14 has a declared `scope` block and no story appears in the tool's own `unassessed` list.

What I could not establish: none within my lane. One item I checked and ruled out — story:session-attachment-lifetime-is-an-accepted-decision cites `sessions.rs` in its context prose but scopes only an ADR and a design doc; its acceptance is doc-only, so it does not actually collide with story:unattached-claimed-session-is-contained (which does scope `app/sessions.rs`) — this is correct as drafted, not a finding. Whether the scope frontmatter's "cited" label for `service.rs` on story:metrics-streams-are-bounded is itself accurate (the body never names that path in prose, only in the frontmatter) is a scope/acceptance-lane question, not mine — noted as out of my lane, does not affect my verdict. Sequencing the 5-way probe.rs/process.rs cluster and the two 2-way pairs is the operator's call, not mine to prescribe.

```findings
- file: .engineering/planning/story/backend-recheck-hashes-only-on-change.md
  line: 14
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: this story and four siblings (story:confined-processes-cannot-nest-user-namespaces, story:daemon-image-serves-exec-or-says-it-cannot, story:exec-oom-kills-the-whole-tree, story:seccomp-denies-af-vsock) all declare crates/substrate-host/src/probe.rs as cited scope, three of them also share crates/substrate-host/src/process.rs, and none of the five bodies or relations names another (cited, all five scope blocks; corroborated by `aep artifact waves`)
- file: .engineering/planning/story/line-patch-edits-apply-in-line-order.md
  line: 14
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: this story and story:directory-listing-survives-special-entries both cite crates/substrate-host/src/fs.rs as scope and neither body mentions the other (cited, both scope blocks; corroborated by `aep artifact waves`)
- file: .engineering/planning/story/metrics-streams-are-bounded.md
  line: 16
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: this story and story:lease-cleanup-reads-exec-state-only both cite crates/substrate-daemon/src/app/service.rs as scope and neither body mentions the other (cited, both scope blocks; corroborated by `aep artifact waves`)
```
