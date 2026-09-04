---
format: aep.planning-md/1
id: review-result:review-2026-09-03-parallel-safety-round-2
kind: review-result
status: active
title: Parallel-safety critic (aep-planning:plan-critic-parallel-safety), round 2
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

What I read: all 14 stories under `decomposes: epic:review-2026-09-03-findings` (`aep artifact list --format json`), each via `aep artifact show <id>`, plus `aep artifact waves --format json` (computes scope-based collisions directly) and the round-1 record `review-result:review-2026-09-03-parallel-safety-round-1`. Verified the 7 shared file paths exist in the tree (`crates/substrate-host/src/{probe,process,fs}.rs`, `crates/substrate-daemon/src/app/service.rs`, `crates/substrate-daemon/src/app/metrics.rs`, `crates/substrate-store/src/execs.rs`, `crates/substrate-wire/src/lib.rs`), and confirmed with `grep -n on_upgrade` that the two session/upgrade stories do not in fact share a file. Surface established: 14 cited, 0 inferred, 0 unplaceable — all 14 appear in `waves`' scoped set, none in its `unassessed` list.

Findings: none. `aep artifact waves` reports exactly 15 collisions across 3 groups (the 5-way `probe.rs`/`process.rs` cluster: story:backend-recheck-hashes-only-on-change, story:confined-processes-cannot-nest-user-namespaces, story:daemon-image-serves-exec-or-says-it-cannot, story:exec-oom-kills-the-whole-tree, story:seccomp-denies-af-vsock; the `fs.rs` pair story:line-patch-edits-apply-in-line-order / story:directory-listing-survives-special-entries; the `service.rs` pair story:metrics-streams-are-bounded / story:lease-cleanup-reads-exec-state-only). Each of these 9 stories now carries a `## Parallel work` section naming the exact shared path(s) and every colliding sibling, matching the tool's collision set pair-for-pair (including the "three of them also share `process.rs`" detail, which is accurate), and directing sequential work. This closes round 1's three findings (all recorded `fixed` in the round-1 outcomes). The remaining 5 stories have no collisions in `waves` and none introduce a new one.

What I could not establish: none within my lane. Out of my lane, not affecting my verdict: story:unattached-claimed-session-is-contained and story:upgraded-connections-keep-their-permit describe themselves in prose as landing on "the same `on_upgrade` seam" and carry an `informed_by` relation — but their declared scopes are different files (`app/sessions.rs` vs `runtime.rs`), and `grep -n on_upgrade crates/substrate-daemon/src/runtime.rs` returns nothing, so there is no file-level collision; whether that conceptual coupling is handled correctly is a design-split question, not a parallel-safety one.

```findings
[]
```
