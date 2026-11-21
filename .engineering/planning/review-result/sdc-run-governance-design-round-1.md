---
format: aep.planning-md/3
id: review-result:sdc-run-governance-design-round-1
kind: review-result
status: active
title: 'Plan critic (design), round 1: runs are governed and limit hits are events'
tags:
- plan-critic
- sdc
relations:
- reviews: story:linux-executable-allow-list
- reviews: story:executable-deny-list
- reviews: story:executable-lists-on-oci-and-pod
- reviews: story:exec-start-carries-executable-lists
- reviews: story:run-observation-names-the-limit-that-ended-it
- reviews: story:audit-records-run-end-and-limit
- reviews: story:process-count-limit-hit-is-observed
- reviews: story:confinement-profile-ingress-routes
revision: 1
---
needs-revision

story:linux-executable-allow-list — its acceptance holds `confinement.exec-subpaths` out of the daemon snapshot, but `linux::proof` and `publish_confinement` are one mechanism that a test ties together, and story:exec-start-carries-executable-lists also claims the publication, so each half of "the daemon publishes the fact" is split across two stories; the allow-list body should take the publication, with the probe test and delegated-lane list updated, or say how `publish_confinement` withholds a proved clause until the wire story — crates/substrate-host/src/probe.rs:1146, crates/substrate-host/src/probe.rs:1210-1217, .engineering/planning/story/linux-executable-allow-list.md:83, .engineering/planning/story/exec-start-carries-executable-lists.md:73

story:process-count-limit-hit-is-observed — its Outcome promises the wire observation unconditionally while step 1 makes the wire optional. Its wire half is a successor bundle, and it names none, so it would collide with the `0.27.0` that story:exec-start-carries-executable-lists claims, with no edge between them. Split the wire half into its own story with `depends_on` edges to both, or drop it from the Outcome — .engineering/planning/story/process-count-limit-hit-is-observed.md:42, .engineering/planning/story/process-count-limit-hit-is-observed.md:68, .engineering/planning/story/exec-start-carries-executable-lists.md:69

story:confinement-profile-ingress-routes — its outcome says the `linux` adapter serves `network_ingress`, but the daemon's family fact is hard-set to `None` in `publish_confinement` and mirrored by the same proof test. The story's scope omits `probe.rs` and says nothing of the delegated vectors' family list, so the snapshot change is unowned; the body should take that change, or say it is withheld and which story lifts it — crates/substrate-host/src/probe.rs:295, crates/substrate-host/src/probe.rs:1199-1205, .engineering/planning/story/confinement-profile-ingress-routes.md:63

**What you read:** the 8 stories plus the epic and `story:inbound-endpoint-for-a-confined-exec`, via `aep plan artifact show`, `relations`, `graph`, `validate` (it returns `valid`, plus unrelated review-result advisories) and `git grep`. I read `probe.rs`, `linux.rs` `proof` and the wire `ConfinementProperty` enum. I walked 14 edges (8 `decomposes`, 6 `depends_on`) and the graph's edges around the set, and found no cycle. I traced 7 acceptance reads to a producer in the set, and an edge records all 7.

**What you could not establish:**
- The bundle-number collision between the two bundle-cutting stories is parallel-safety, and I did not let it set the verdict. Only the Outcome-versus-optional wire shape in the process-count finding is mine.
- Whether Landlock can express a deny beneath an allowed directory is an unease and not a finding. The allow-list design picks its mechanism before the deny-list design states its needs, and both land in one document.
- I did not check whether the design-document numbers 35 and 36 are free beyond `docs/design` ending at 34 and no other planning artifact naming them.

```findings
- file: .engineering/planning/story/linux-executable-allow-list.md
  line: 83
  category: design
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "its acceptance holds confinement.exec-subpaths out of the daemon snapshot, but linux::proof and publish_confinement are one mechanism tied by the_published_set_is_the_linux_adapters_proof and story:exec-start-carries-executable-lists also claims the publication, so the allow-list body should own the publication (probe test and delegated-lane list updated) or say how publish_confinement withholds a proved clause until the wire story"
- file: .engineering/planning/story/process-count-limit-hit-is-observed.md
  line: 42
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "its Outcome promises the wire observation unconditionally while step 1 makes the wire optional, and its wire half is a successor bundle that names no version and would collide with the 0.27.0 story:exec-start-carries-executable-lists claims, with no edge between them; split the wire half into its own story with depends_on edges to both, or drop it from the Outcome"
- file: .engineering/planning/story/confinement-profile-ingress-routes.md
  line: 63
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "its outcome says the linux adapter serves network_ingress, but the daemon's family fact is hard-set to None in publish_confinement and mirrored by the same proof test, and the story's scope omits probe.rs and the delegated vectors' family list, so the snapshot change is unowned; the body should take it or say which story lifts it"
```
