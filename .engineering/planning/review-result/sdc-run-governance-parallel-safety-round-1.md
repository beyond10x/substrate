---
format: aep.planning-md/3
id: review-result:sdc-run-governance-parallel-safety-round-1
kind: review-result
status: active
title: 'Plan critic (parallel-safety), round 1: runs are governed and limit hits are events'
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

story:run-observation-names-the-limit-that-ended-it — its `linux.rs`, `confinement.yaml`, scenarios and suite (cited, `:101-110`) are also claimed by story:linux-executable-allow-list and story:executable-deny-list, its `oci.rs` and `pod.rs` by story:executable-lists-on-oci-and-pod, and its `macos.rs` by story:executable-deny-list. It has no `depends_on` edge to any of them, and the epic's one-at-a-time rule is prose, not an edge, so `waves` can schedule it beside them; add an edge to story:executable-lists-on-oci-and-pod that records the shared files as its reason, or split the surface so they no longer share it — .engineering/planning/story/run-observation-names-the-limit-that-ended-it.md:13, .engineering/planning/epic/runs-are-governed-and-limit-hits-are-events.md:85
story:confinement-profile-ingress-routes — it lands on `profile.rs`, `facts.rs`, `mod.rs`, `linux.rs` and `confinement.yaml` (all cited, `:84-93`), which story:executable-deny-list (`profile.rs:43-97` against this story's `:93-96`, and `facts.rs`), story:run-observation-names-the-limit-that-ended-it, story:audit-records-run-end-and-limit and story:process-count-limit-hit-is-observed also change. Its only edge is to story:inbound-endpoint-for-a-confined-exec, and the epic's § Order places it eighth in prose only; add an ordering edge that records the shared files, or split the surface — .engineering/planning/story/confinement-profile-ingress-routes.md:13, .engineering/planning/epic/runs-are-governed-and-limit-hits-are-events.md:85
story:process-count-limit-hit-is-observed — if the design picks a wire change it claims `crates/substrate-wire/src/lib.rs` (inferred), `xtask/bundle-source` and `contracts/substrate-wire` (inferred), and story:exec-start-carries-executable-lists already claims `lib.rs` (cited) and `0.27.0` under both bundle paths. Neither body names the other or says which one cuts `0.27.0` (those directories do not exist yet), `process.rs` is shared too, and the two sit on different dependency chains with no edge between them; add an edge recording the shared bundle and files, or split — .engineering/planning/story/process-count-limit-hit-is-observed.md:105, .engineering/planning/story/exec-start-carries-executable-lists.md:100
story:exec-start-carries-executable-lists — its new document is scoped as "`docs/design` or `adr` (new document)" with no path, and `adr/README.md` is not in its scope, so the next ADR number and the README index row it would share with the ADR of story:confinement-profile-ingress-routes (`adr/README.md` in that story's body) cannot be assessed; name the file, or say it is a design document numbered after 36 — .engineering/planning/story/exec-start-carries-executable-lists.md:100, .engineering/planning/story/confinement-profile-ingress-routes.md:86
story:executable-deny-list — its acceptance updates the committed public-items list, which is `crates/substrate-host/tests/confinement-public-items.txt` (cited, its header names the regenerate command). Its typed scope omits that file, though story:run-observation-names-the-limit-that-ended-it and story:audit-records-run-end-and-limit also bless it, so `waves` cannot see that shared file; add it to the scope — .engineering/planning/story/executable-deny-list.md:86, crates/substrate-host/tests/confinement-public-items.txt:1
story:run-observation-names-the-limit-that-ended-it — its acceptance blesses the same public-items list, and its typed scope omits `crates/substrate-host/tests/confinement-public-items.txt`; add it — .engineering/planning/story/run-observation-names-the-limit-that-ended-it.md:91
story:audit-records-run-end-and-limit — its acceptance blesses the same public-items list, and its typed scope omits `crates/substrate-host/tests/confinement-public-items.txt`. It also omits `spec/confinement-suite.json` and `spec/confinement/scenarios`, which `check-ess` regenerates when it edits `confinement.yaml` and which the other `confinement.yaml` stories list; add them — .engineering/planning/story/audit-records-run-end-and-limit.md:83

What I read: 8 stories and the epic, with `aep plan artifact show` on each, `aep plan artifact waves --kind story --status draft` and `aep plan artifact list --format json`. I also read `crates/substrate-host/tests/confinement-public-items.txt`, its test, `spec/confinement/README.md` and the directory listings of `docs/design`, `adr` and `spec`.

What I could not establish:
- **Surfaces established:** cited 8 items (each has at least one cited path), inferred 0 as their whole scope, unplaced 0 outright. One scope entry is unplaceable: the new document of story:exec-start-carries-executable-lists.
- **Acceptance reads traced to a producer in the set:** 8 (the design documents 35 and 36, the audit record reading the limit, the process-count hit reading the audit record, the deny-list reading the allow-list, and the oci/pod and exec-start stories reading the deny-list), and `depends_on` records 8. The ninth read, the ingress story reading the endpoint mechanism, has its producer outside the set and an edge.
- **Out of my lane, not counted:** the epic's sentence listing stories 1, 2, 5, 6 and 7 as the shared-file set omits stories 3 and 8, which touch `confinement.yaml` too. That is for the scope or design critic.
- **Not assessed:** the collisions with story:host-process-container-records and story:host-process-adopt that `waves` reports, because they are outside the set I was given.

```findings
- file: .engineering/planning/story/run-observation-names-the-limit-that-ended-it.md
  line: 13
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "lands on linux.rs, confinement.yaml, scenarios and suite shared with story:linux-executable-allow-list and story:executable-deny-list, on oci.rs and pod.rs shared with story:executable-lists-on-oci-and-pod and on macos.rs shared with story:executable-deny-list, with no depends_on edge; the epic's one-at-a-time order is prose only"
- file: .engineering/planning/story/confinement-profile-ingress-routes.md
  line: 13
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "lands on profile.rs, facts.rs, mod.rs, linux.rs and confinement.yaml, all cited and also changed by story:executable-deny-list, story:run-observation-names-the-limit-that-ended-it, story:audit-records-run-end-and-limit and story:process-count-limit-hit-is-observed, and its only edge is to story:inbound-endpoint-for-a-confined-exec"
- file: .engineering/planning/story/process-count-limit-hit-is-observed.md
  line: 105
  category: parallel-safety
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "its conditional wire change claims substrate-wire lib.rs and the bundle source and contracts paths (inferred) that story:exec-start-carries-executable-lists also claims for 0.27.0 (files not yet created), and process.rs, with no edge between the two chains and neither body naming the other or which one cuts 0.27.0"
- file: .engineering/planning/story/exec-start-carries-executable-lists.md
  line: 100
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "its new document is scoped as 'docs/design or adr' with no path and adr/README.md is not in scope, so the ADR number and index row it may share with story:confinement-profile-ingress-routes cannot be assessed (inferred)"
- file: .engineering/planning/story/executable-deny-list.md
  line: 86
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "acceptance updates crates/substrate-host/tests/confinement-public-items.txt, which story:run-observation-names-the-limit-that-ended-it and story:audit-records-run-end-and-limit also bless, but it is not in the typed scope so waves cannot see the shared file"
- file: .engineering/planning/story/run-observation-names-the-limit-that-ended-it.md
  line: 91
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "acceptance updates crates/substrate-host/tests/confinement-public-items.txt, shared with story:executable-deny-list and story:audit-records-run-end-and-limit, and the typed scope omits it"
- file: .engineering/planning/story/audit-records-run-end-and-limit.md
  line: 83
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "acceptance updates crates/substrate-host/tests/confinement-public-items.txt and the story edits confinement.yaml, yet its typed scope omits that file, spec/confinement-suite.json and spec/confinement/scenarios, which check-ess regenerates and the sibling stories list"
```
