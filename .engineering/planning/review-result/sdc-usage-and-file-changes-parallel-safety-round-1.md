---
format: aep.planning-md/3
id: review-result:sdc-usage-and-file-changes-parallel-safety-round-1
kind: review-result
status: active
title: 'Plan critic (parallel-safety), round 1: usage on the observation and the file-change record'
tags:
- plan-critic
- sdc
relations:
- reviews: story:run-observation-carries-usage
- reviews: story:exec-file-changes-are-recorded
revision: 1
---
needs-revision

story:run-observation-carries-usage — it and story:executable-lists-on-oci-and-pod both land on `spec/confinement/confinement.yaml`, `spec/confinement/scenarios` and the regenerated `spec/confinement-suite.json` (cited in the other, inferred here), no edge orders them, and neither § Order names the other; the plan needs an edge recording the shared spec files as its reason, or the two stories' spec changes split — .engineering/planning/story/run-observation-carries-usage.md:107, .engineering/planning/story/executable-lists-on-oci-and-pod.md:95
story:run-observation-carries-usage — it and story:confinement-profile-ingress-routes both land on `crates/substrate-host/src/confinement/linux.rs`, `confinement/mod.rs` and `tests/confinement-public-items.txt` (cited in both bodies), no edge orders them, and the ingress story orders itself only after "every other story of the epic" while this story states it is "not part of it"; the plan needs an edge recording those files as its reason, or the surface split — .engineering/planning/story/run-observation-carries-usage.md:109, .engineering/planning/story/confinement-profile-ingress-routes.md:122
story:run-observation-carries-usage — it and story:exec-observation-reports-process-count-hits both add a section to `docs/design/36-limit-hits.md` (inferred; the file is not in the tree yet, story:run-observation-names-the-limit-that-ended-it creates it), both depend only on story:process-count-limit-hit-is-observed, and neither § Order names the other; the plan needs an edge recording the design file as its reason, or the sections split — .engineering/planning/story/run-observation-carries-usage.md:107, .engineering/planning/story/exec-observation-reports-process-count-hits.md:108
story:exec-file-changes-are-recorded — it and story:confinement-profile-ingress-routes both land on `crates/substrate-host/src/confinement/mod.rs` (cited in both bodies), `crates/substrate-host/src/probe.rs`, `crates/substrate-daemon/tests/runtime_vectors.rs`, `AGENTS.md` and `adr` (inferred in this one), no edge orders them, and the ingress story's "every other story of the epic" excludes this story, which says it is "not part of it"; the plan needs an edge recording the shared files as its reason, or the surface split — .engineering/planning/story/exec-file-changes-are-recorded.md:166, .engineering/planning/story/confinement-profile-ingress-routes.md:122
story:exec-file-changes-are-recorded — it cuts a successor bundle and lands on `crates/substrate-wire/src/lib.rs` (cited in both bodies), `xtask/bundle-source`, `xtask/src/bundle.rs`, `scripts/gate.sh`, `crates/substrate-host/src/process.rs` and `crates/substrate-daemon/tests/runtime_vectors.rs` with story:exec-start-carries-executable-lists and story:exec-observation-reports-process-count-hits, and its "next free version" predecessor is the bundle those two cut, yet it has no `depends_on` edge to either and its § Order names neither (those two Orders order only each other); the missing edge is `depends_on story:exec-observation-reports-process-count-hits`, which already follows exec-start, or the stories merge — .engineering/planning/story/exec-file-changes-are-recorded.md:130, .engineering/planning/story/exec-file-changes-are-recorded.md:170, .engineering/planning/story/exec-observation-reports-process-count-hits.md:108
story:exec-file-changes-are-recorded — its scope claims the whole `spec` directory (inferred; the body says "a new domain or `spec/confinement`") and so collides with every spec edit in the set, including story:executable-lists-on-oci-and-pod on `spec/confinement-suite.json`; the scope should name `spec/confinement` or the new domain's path once the ADR decides it — .engineering/planning/story/exec-file-changes-are-recorded.md:44, .engineering/planning/story/exec-file-changes-are-recorded.md:122

What I read: 2 stories under review plus the epic's six other draft or active stories, via `aep plan artifact show` on each, `aep plan artifact list --format json`, `aep plan artifact waves --kind story` and `git grep`, plus `crates/substrate-host/src/confinement/mod.rs:696-706` and `crates/substrate-wire/src/lib.rs:2161-2185`.

Surfaces: 2 items placed from cited surfaces, 0 inferred only, 0 unplaced. Of the acceptance reads, 4 traced to a producer outside the pair, and a `depends_on` path records 3 of them (`run_ended` record, the limit, and the audit sink, all through story:process-count-limit-hit-is-observed). The fourth, the bundle predecessor, has no edge and is the fifth finding. The edge from story:exec-file-changes-are-recorded to story:run-observation-carries-usage is recorded and its § Order states it.

Could not establish:
- Whether `docs/design/36-limit-hits.md` has a single creator: it is not in the tree, and I read story:run-observation-names-the-limit-that-ended-it as creating it.
- Out of my lane: story:exec-file-changes-are-recorded combines an ADR, an ESS model, an in-process path and a wire bundle in one story, which is a split question for the design critic.
- `website/docs` and `AGENTS.md` appear as inferred scope on most stories, so their collisions in `aep plan artifact waves` are noise I did not report.

```findings
- file: .engineering/planning/story/run-observation-carries-usage.md
  line: 107
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "it and story:executable-lists-on-oci-and-pod both land on spec/confinement/confinement.yaml, spec/confinement/scenarios and spec/confinement-suite.json (cited in the other, inferred here, so the claim is weaker), no edge orders them and neither Order names the other; add an edge recording the shared spec files as its reason, or split the surface"
- file: .engineering/planning/story/run-observation-carries-usage.md
  line: 109
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "it and story:confinement-profile-ingress-routes both land on confinement/linux.rs, confinement/mod.rs and confinement-public-items.txt (cited in both bodies), no edge orders them, and the ingress Order covers only stories of the epic while this story says it is not part of it; add an edge recording those files as its reason, or split the surface"
- file: .engineering/planning/story/run-observation-carries-usage.md
  line: 107
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "it and story:exec-observation-reports-process-count-hits both add a section to docs/design/36-limit-hits.md (inferred; the file does not exist yet), both depend only on story:process-count-limit-hit-is-observed and neither Order names the other; add an edge recording the design file as its reason, or split the sections"
- file: .engineering/planning/story/exec-file-changes-are-recorded.md
  line: 166
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "it and story:confinement-profile-ingress-routes both land on confinement/mod.rs (cited in both bodies) and on probe.rs, runtime_vectors.rs, AGENTS.md and adr (inferred in this one), no edge orders them and the ingress Order covers only stories of the epic while this story says it is not part of it; add an edge recording the shared files as its reason, or split the surface"
- file: .engineering/planning/story/exec-file-changes-are-recorded.md
  line: 170
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "it cuts a successor bundle and lands on crates/substrate-wire/src/lib.rs (cited in both bodies), xtask/bundle-source, xtask/src/bundle.rs and scripts/gate.sh with story:exec-start-carries-executable-lists and story:exec-observation-reports-process-count-hits, and its next-free-version predecessor is the bundle those two cut, yet it has no depends_on edge to either and its Order names neither; add depends_on story:exec-observation-reports-process-count-hits (which already follows exec-start) or merge the stories"
- file: .engineering/planning/story/exec-file-changes-are-recorded.md
  line: 44
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "its scope claims the whole spec directory (inferred), which collides with every spec edit in the set, including story:executable-lists-on-oci-and-pod on spec/confinement-suite.json; name spec/confinement or the new domain's path once the ADR decides it"
```
