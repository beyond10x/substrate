---
format: aep.planning-md/3
id: review-result:sdc-run-governance-acceptance-round-1
kind: review-result
status: active
title: 'Plan critic (acceptance), round 1: runs are governed and limit hits are events'
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

story:executable-lists-on-oci-and-pod — the acceptance is "either runs with a denied execution failing inside the run, or is refused `confinement.fact-unserved`", and the refused branch is already true for both adapters today, so the story can close with no adapter change and no transition is named — .engineering/planning/story/executable-lists-on-oci-and-pod.md:64
story:exec-start-carries-executable-lists — the Outcome promises the lists "on an exec start and on a session start", but the acceptance and every check exercise only the exec start, so a session start can ship with no field and no refusal — .engineering/planning/story/exec-start-carries-executable-lists.md:49
story:exec-start-carries-executable-lists — the Boundary "The Kubernetes namespace driver keeps the properties absent and refuses the fields by name" has no check (the namespace lane's `NAMESPACE_CASES` is not named), so it cannot be seen to hold — .engineering/planning/story/exec-start-carries-executable-lists.md:96
story:run-observation-names-the-limit-that-ended-it — the Outcome names four limits (memory, CPU time, wall time, output bytes) but the acceptance runs only memory, CPU and timeout, so the output-bytes limit can go unobserved while the story closes — .engineering/planning/story/run-observation-names-the-limit-that-ended-it.md:80
story:run-observation-names-the-limit-that-ended-it — the Outcome "`memory_oom_kills` is reported by every adapter that can read it" has no acceptance or check; the only related check, "each run the cases their design row lists", defers the case list to a document that does not exist yet — .engineering/planning/story/run-observation-names-the-limit-that-ended-it.md:46
story:audit-records-run-end-and-limit — the Outcome says the embedder learns of the limit "without polling", but the acceptance does not require a run nobody waits on or observes, so a record written only on `observe` or `wait` passes — .engineering/planning/story/audit-records-run-end-and-limit.md:73
story:process-count-limit-hit-is-observed — the Outcome states the wire result unconditionally ("an exec that hit the bound says so on its observation without the client having asked for measurements") while the only wire check is conditional on the design choosing a wire change, so the story is done with that outcome false if the design leaves the wire alone — .engineering/planning/story/process-count-limit-hit-is-observed.md:42
story:confinement-profile-ingress-routes — the family-refusal check lists `local`, `macos`, `oci` or `pod` and omits `systemd`, whose answer depends on the adapter it wraps (`linux`'s set versus `local`), so `systemd` around `local` is left unchecked — .engineering/planning/story/confinement-profile-ingress-routes.md:74

What I read: all 8 given ids in full with `aep plan artifact show`, plus `story:confinement-consumer-surface` and `aep plan artifact validate`, which returned `valid`. In the tree I checked `probe.rs:1170-1225`, `linux.rs:20-35`, `confinement.yaml:195-225`, `scripts/delegated-lane.sh`, the workflows and the tests directory.

What I could not establish:
- Whether `docs/design/35` and `36` or the new ADR can state the answers the acceptances leave to them. They do not exist yet.
- Whether the "daemon snapshot does not gain `confinement.exec-subpaths`" check in `story:linux-executable-allow-list` is satisfiable. The probe test pins the daemon's set to three properties, and the daemon's snapshot appears to be built separately from the `linux` adapter's `Fact` set. I did not trace that coupling, and it is a design-lane question.
- Whether the "`oci` lane" and "`pod` lane" named in several acceptances map to runnable lanes. I found `ess_confinement_oci.rs` and `ess_confinement_pod.rs` but no workflow or script for them.
- Out of my lane: dependency ordering and coupling between the eight stories (design critic), and whether the epic's relayed requirement is fully covered (scope critic). Neither set my verdict.

```findings
- file: .engineering/planning/story/executable-lists-on-oci-and-pod.md
  line: 64
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the acceptance is 'either runs with a denied execution failing inside the run, or is refused confinement.fact-unserved', and the refused branch is already true for both adapters today, so the story can close with no adapter change and no transition is named"
- file: .engineering/planning/story/exec-start-carries-executable-lists.md
  line: 49
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the Outcome promises the lists on an exec start and on a session start, but the acceptance and every check exercise only the exec start, so a session start can ship with no field and no refusal"
- file: .engineering/planning/story/exec-start-carries-executable-lists.md
  line: 96
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the Boundary that the Kubernetes namespace driver keeps the properties absent and refuses the fields by name has no check (the namespace lane's NAMESPACE_CASES is not named), so it cannot be seen to hold"
- file: .engineering/planning/story/run-observation-names-the-limit-that-ended-it.md
  line: 80
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the Outcome names four limits (memory, CPU time, wall time, output bytes) but the acceptance runs only memory, CPU and timeout, so the output-bytes limit can go unobserved while the story closes"
- file: .engineering/planning/story/run-observation-names-the-limit-that-ended-it.md
  line: 46
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the Outcome 'memory_oom_kills is reported by every adapter that can read it' has no acceptance or check; the only related check, 'each run the cases their design row lists', defers the case list to a document that does not exist yet"
- file: .engineering/planning/story/audit-records-run-end-and-limit.md
  line: 73
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the Outcome says the embedder learns of the limit without polling, but the acceptance does not require a run nobody waits on or observes, so a record written only on observe or wait passes"
- file: .engineering/planning/story/process-count-limit-hit-is-observed.md
  line: 42
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the Outcome states the wire result unconditionally (an exec that hit the bound says so on its observation without measurements) while the only wire check is conditional on the design choosing a wire change, so the story is done with that outcome false if the design leaves the wire alone"
- file: .engineering/planning/story/confinement-profile-ingress-routes.md
  line: 74
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the family-refusal check lists local, macos, oci or pod and omits systemd, whose answer depends on the adapter it wraps (linux's set versus local), so systemd around local is left unchecked"
```
