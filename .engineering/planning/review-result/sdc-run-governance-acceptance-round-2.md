---
format: aep.planning-md/3
id: review-result:sdc-run-governance-acceptance-round-2
kind: review-result
status: active
title: 'Plan critic (acceptance), round 2: runs are governed and limit hits are events'
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
story:executable-lists-on-oci-and-pod — the expected refusal contradicts story:executable-deny-list: for a profile naming both lists, this story's scenario expects `confinement.fact-unserved` naming `exec_subpaths`, while the deny-list story's check expects it to name "the new fact" on `local`, which also proves neither fact. The refusal names only the first unproved required fact in `FACT_ORDER`, so the two checks cannot both pass, and the scenario's expected value is undetermined until the design fixes the order. The body should name the fact the design's order produces — `.engineering/planning/story/executable-lists-on-oci-and-pod.md:60` (against `.engineering/planning/story/executable-deny-list.md:95-96`; `crates/substrate-host/src/confinement/mod.rs:18`, `:191`)
story:executable-lists-on-oci-and-pod — the acceptance is one sentence joining two independent outcomes with "and": the design document records a mechanism, command and result for each adapter, and each lane runs the refusal scenario. One can pass while the other fails — `.engineering/planning/story/executable-lists-on-oci-and-pod.md:68-71`
story:run-observation-names-the-limit-that-ended-it — the What-changes items that say `systemd` reports the timeout and CPU budget and `pod` reports what its container status states have no check. The only `systemd` check is the memory case, and no check mentions `pod`. Those two behaviours could be left unbuilt and every listed check would still pass — `.engineering/planning/story/run-observation-names-the-limit-that-ended-it.md:82-85` (checks at `:93-107`)

**What I read:** 9 of 9 ids, with `aep plan artifact show` on each. I also checked these in the tree:
- `ess_confinement_systemd.rs` exists.
- `SUBSTRATE_OCI_LANE` and `SUBSTRATE_POD_LANE` exist in the host tests.
- `macos.yml` runs on `macos-15`.
- `delegated-lane.sh` runs `ess_confinement_linux`.
- The first-unproved-fact refusal rule is in `confinement/mod.rs`.

**What I could not establish:**
- Whether `exec_subpaths` precedes the new deny-list fact in the `Fact` order. Design 35 has not been written, so I can't say which of the two contradicting checks is wrong.
- Whether the scenarios in the yaml are authored yet. They are not, and the stories are at the start of their lifecycle.
- Out of my lane, not counted in the verdict: `story:confinement-profile-ingress-routes` depends on `story:inbound-endpoint-for-a-confined-exec`, which is draft and has not decided the mechanism.

```findings
- file: .engineering/planning/story/executable-lists-on-oci-and-pod.md
  line: 60
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the scenario expects confinement.fact-unserved naming exec_subpaths for a profile with both lists, while story:executable-deny-list expects the refusal to name the new fact on local, which also proves neither; the refusal names only the first unproved fact in FACT_ORDER, so both checks cannot pass and the expected value is undetermined"
- file: .engineering/planning/story/executable-lists-on-oci-and-pod.md
  line: 68
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the acceptance is one sentence joining two independent outcomes with 'and': the design records a mechanism, command and result for each adapter, and each lane runs the refusal scenario"
- file: .engineering/planning/story/run-observation-names-the-limit-that-ended-it.md
  line: 82
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the What-changes items that say systemd reports the timeout and CPU budget and pod reports its container status have no check; the only systemd check is the memory case and no check mentions pod"
```
