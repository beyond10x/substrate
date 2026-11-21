---
format: aep.planning-md/3
id: review-result:sdc-run-governance-design-round-2
kind: review-result
status: active
title: 'Plan critic (design), round 2: runs are governed and limit hits are events'
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
- reviews: story:exec-observation-reports-process-count-hits
revision: 1
---
needs-revision

story:exec-observation-reports-process-count-hits — its step 3 claims a host-driver read of `pids.events` when an exec ends without measurements, but the in-process story needs that same read for its own acceptance and decides how it gets the count, so both bodies claim one change. The fix is for the body to say story:process-count-limit-hit-is-observed owns the read and the host-observation carrier, and this story only exposes that value as the wire member. — .engineering/planning/story/exec-observation-reports-process-count-hits.md:79, .engineering/planning/story/process-count-limit-hit-is-observed.md:67, crates/substrate-host/src/process.rs:3128

story:run-observation-names-the-limit-that-ended-it — it promises `memory_oom_kills` of at least 1 on `linux`, but the host driver reads the OOM counter of an unmeasured run into a local (`unmeasured_oom_kills`) that never reaches `ExecObservation`. The cgroup is removed at `reconcile_cgroup`, so the adapter has nothing to read, and the scope names no `process.rs` change. The body must either claim the same carrier as stories 5 and 8 (that story would then edit `process.rs`) or drop the `linux` count. — .engineering/planning/story/run-observation-names-the-limit-that-ended-it.md:96, crates/substrate-host/src/process.rs:3128, crates/substrate-host/src/observation.rs:20

story:exec-start-carries-executable-lists — it fixes the successor bundle at `0.27.0` in its title, scope and checks. `story:inbound-endpoint-for-a-confined-exec`, which cuts its own successor bundle, is unordered against this story: the epic lists it after story 8 but no edge records that, so `aep plan artifact waves` can schedule it first. The body should name the next free version at start, as story:exec-observation-reports-process-count-hits already does. — .engineering/planning/story/exec-start-carries-executable-lists.md:78, .engineering/planning/story/exec-observation-reports-process-count-hits.md:77, .engineering/planning/story/inbound-endpoint-for-a-confined-exec.md:56

What I read: all nine stories, the epic, and the two context stories in full. I ran `aep plan artifact relations`, `validate` and `waves`, plus the story edges. The graph has 13 declared edges, 12 inside the set and 1 outside it (to `story:inbound-endpoint-for-a-confined-exec`), and I found no cycle. I read the code the stories cite in `process.rs`, `observation.rs`, `linux.rs`, `facts.rs` and `probe.rs`. Of the 12 acceptance reads of a sibling's output that I traced, all 12 have a `depends_on` edge, direct or transitive.

What I could not establish:
- Whether story:linux-executable-allow-list can carry its probe result from `probe.rs` into `linux::proof` without adding a `CapabilityFacts` field that reaches the wire snapshot. Story 1 forbids a wire change, and its design document is meant to decide this, so it is a risk and not a finding.
- The main chain (stories 1 to 5, then 8) is nearly linear. Each ordering edge states a shared-file reason and the epic § Order records it as deliberate, so I did not report it. The three tracks (executable lists, limit hits, wire) are independent in content. The trade-off is to keep the edges for `spec/confinement/confinement.yaml` and the other shared files, or split those surfaces. Parallel safety owns that call.
- Whether the story 7 and 8 acceptance checks are executable is for the acceptance critic.

```findings
- file: .engineering/planning/story/exec-observation-reports-process-count-hits.md
  line: 79
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "its step 3 claims a host-driver read of pids.events when an exec ends without measurements, but the in-process story needs that same read for its own acceptance and decides how it gets the count, so both bodies claim one change. The fix is for the body to say story:process-count-limit-hit-is-observed owns the read and the host-observation carrier, and this story only exposes that value as the wire member."
- file: .engineering/planning/story/run-observation-names-the-limit-that-ended-it.md
  line: 96
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "it promises memory_oom_kills of at least 1 on linux, but the host driver reads the OOM counter of an unmeasured run into a local (unmeasured_oom_kills) that never reaches ExecObservation. The cgroup is removed at reconcile_cgroup, so the adapter has nothing to read, and the scope names no process.rs change. The body must either claim the same carrier as stories 5 and 8 or drop the linux count."
- file: .engineering/planning/story/exec-start-carries-executable-lists.md
  line: 78
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "it fixes the successor bundle at 0.27.0 in its title, scope and checks, but story:inbound-endpoint-for-a-confined-exec, which cuts its own successor bundle, is unordered against it and aep plan artifact waves can schedule it first. The body should name the next free version at start, as story:exec-observation-reports-process-count-hits does."
```
