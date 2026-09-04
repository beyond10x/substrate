---
format: aep.planning-md/1
id: review-result:review-2026-09-03-design-round-2
kind: review-result
status: active
title: Design critic (aep-planning:plan-critic-design), round 2
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

**What I read:** the epic and all 14 stories bearing `decomposes: epic:review-2026-09-03-findings` (`aep artifact show` on all 15 ids, full body), `aep artifact relations` (13 relation kinds), the full `aep artifact graph` dump (183 lines, all 78 artifacts in the store — walked every edge, not only the ones touching the 14 ids), and `aep artifact validate`. Within the 14-story set the only edges are 14 `decomposes → epic:review-2026-09-03-findings` and one `informed_by`: `story:unattached-claimed-session-is-contained → story:upgraded-connections-keep-their-permit`. No `depends_on` among the 14, so no cycle and no serialising chain in the declared graph.

The round-1 finding is resolved. `story:unattached-claimed-session-is-contained` now carries the `informed_by` edge and its Notes name the shared seam explicitly ("This is the same hyper upgrade hand-off seam story:upgraded-connections-keep-their-permit observes first (`informed_by`); read its observation before choosing, and if that story moves the permit into the upgrade task, put the claim there too so the two changes are one edit to `on_upgrade`"). `story:upgraded-connections-keep-their-permit`'s Notes reciprocally name the same story and seam ("story:unattached-claimed-session-is-contained lands on the same `on_upgrade` seam for the session attach route and is `informed_by` this story; whichever remedy is chosen here decides where that story puts its claim"). Both halves now disclose the coupling, its direction, and the possible single-edit outcome, and an edge records it — the defect round 1 named (no edge, no cross-reference) no longer holds.

I checked the remaining 12 stories for the same three defects (split abstraction, hidden dependency, serialising chain) and found none: each declares exactly one outcome, one acceptance, and one file-set; the five stories sharing `probe.rs` (`story:confined-processes-cannot-nest-user-namespaces`, `story:seccomp-denies-af-vsock`, `story:exec-oom-kills-the-whole-tree`, `story:daemon-image-serves-exec-or-says-it-cannot`, `story:backend-recheck-hashes-only-on-change`) each add an independently-testable, self-contained check rather than two halves of one check, and the pairs sharing `fs.rs` and `service.rs` touch disjoint functions with independent acceptances.

**What I could not establish:** whether `informed_by` (versus `depends_on`) is the exactly-right relation kind for the upgraded-connections/unattached-claimed-session pair — the body text ("read its observation before choosing") maps closely to the CLI's stated meaning of `informed_by`, so I did not treat the choice as a defect, but I cannot rule out a stronger ordering constraint once the remedy is actually picked. Also out of my lane, unchanged from round 1: whether the five `probe.rs` stories and the `fs.rs`/`service.rs` pairs may be worked concurrently is plan-critic-parallel-safety's question, not mine — each story's own "Parallel work" section already asserts they are sequenced, which is a claim that critic should check, not one I re-judge here.

```findings
[]
```
