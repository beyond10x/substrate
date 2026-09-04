---
format: aep.planning-md/1
id: review-result:review-2026-09-03-design-round-1
kind: review-result
status: active
title: Design critic (aep-planning:plan-critic-design), round 1
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

Wait — reconsidering, my analysis found a real finding, so the verdict is `needs-revision`.

**Read:** the epic and all 14 stories bearing `decomposes: epic:review-2026-09-03-findings` (`aep artifact show` on each of the 15 ids), `aep artifact relations`, `aep artifact graph` (full dump, 154 lines), and `aep artifact validate`.

**Verdict:** needs-revision

story:unattached-claimed-session-is-contained — its Context and Notes describe the same hyper connection/upgrade hand-off gap that story:upgraded-connections-keep-their-permit's Context names ("hyper 1.11.1 … resolves an upgradeable connection future when it hands the socket to the upgrade, so the permit is released the moment a WebSocket upgrade succeeds", story/upgraded-connections-keep-their-permit.md:22-24), and both stories independently propose the identical remedy at that seam ("move it into the upgrade task", story/upgraded-connections-keep-their-permit.md:37-38, vs. "claim inside the upgrade task", story/unattached-claimed-session-is-contained.md:35), yet no edge records that the two touch one seam — `aep artifact graph` — reasonstory:upgraded-connections-keep-their-permit states every listener, including the session-attach listener, is wrapped by the same `enforce_connection_lifetime` (story/upgraded-connections-keep-their-permit.md:19-20), and story:unattached-claimed-session-is-contained's own bug ("If the client drops between the `101` and the upgrade, the `on_upgrade` closure never runs, the permit is dropped", story/unattached-claimed-session-is-contained.md:21-22) is the identical timing window described from the other side.

**What I read:** 1 epic + 14 stories, `aep artifact relations` (13 relation kinds), `aep artifact graph` (full store, 154 lines — walked all edges, not just ones touching the 14 ids, to check for cycles or inbound edges from outside the set), `aep artifact validate`. Within the 14-story set every item declares exactly one edge (`decomposes` → the epic); no `depends_on` among them, so there is no cycle and no serialising chain in the declared graph. The only inbound edges to the 14 beyond the epic are 14 `reviews` edges from `review-result:review-2026-09-03-scope-round-1`, which is a review record, not a work-order edge.

**What I could not establish:** whether a fix to the shared upgrade-task seam (finding above) would actually require sequencing rather than just cross-reading — I read this as an ordering risk worth a `depends_on` or at minimum `informed_by` edge, but cannot rule out that the two remedies land in disjoint code paths once written. Also out of my lane: five stories (backend-recheck-hashes-only-on-change, confined-processes-cannot-nest-user-namespaces, daemon-image-serves-exec-or-says-it-cannot, exec-oom-kills-the-whole-tree, seccomp-denies-af-vsock) all cite `probe.rs` and at least two (confined-processes, seccomp-denies-af-vsock) both extend what reads like the same sentinel-case list — each addition is self-sufficient on its own, so I did not treat it as a split abstraction, but whether five stories may be worked on that file concurrently is plan-critic-parallel-safety's question, not mine.

```findings
- file: story/unattached-claimed-session-is-contained.md
  line: 35
  category: design
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: its Context and Notes describe the same hyper connection/upgrade hand-off gap that story:upgraded-connections-keep-their-permit's Context names, and both stories independently propose the identical remedy ("move it into the upgrade task") at that seam, yet no edge records that the two touch one seam
```
