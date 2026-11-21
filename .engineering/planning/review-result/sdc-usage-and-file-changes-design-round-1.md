---
format: aep.planning-md/3
id: review-result:sdc-usage-and-file-changes-design-round-1
kind: review-result
status: active
title: 'Plan critic (design), round 1: usage on the observation and the file-change record'
tags:
- plan-critic
- sdc
relations:
- reviews: story:run-observation-carries-usage
- reviews: story:exec-file-changes-are-recorded
revision: 1
---
needs-revision

story:run-observation-carries-usage — its body never says how usage reaches the adapter, so two carriers now exist for the same counters: the host-internal end-of-run member that `story:run-observation-names-the-limit-that-ended-it` and `story:process-count-limit-hit-is-observed` own (OOM count, `pids.events` max, both read for every exec "measured or not"), and `ExecObservation.resource.usage`, which a measured start fills with `ResourceUsage` carrying the same `memory_oom_kills` and `process_limit_hits`. Fix: the body states whether the usage figures extend that member or the `linux` profile asks for measurements, and which one the member then replaces. Add `process.rs` and `observation.rs` to its scope if it does the former. — .engineering/planning/story/run-observation-carries-usage.md:64, :76, :82; crates/substrate-host/src/process.rs:3115-3128; crates/substrate-host/src/observation.rs:19-34; crates/substrate-wire/src/lib.rs:2171-2175

story:exec-file-changes-are-recorded — step 3 has the audit sink receive "a record" but no ADR decision says whether that is a member of the `run_ended` record, which `docs/design/36-limit-hits.md` and `story:audit-records-run-end-and-limit` own, or a new `AuditRecord` variant. The story's ADR and that design document would both own the sink contract. Fix: add the sink record's shape (member or variant, and its size bound) to the ADR's decision list. — .engineering/planning/story/exec-file-changes-are-recorded.md:128, :105-121; .engineering/planning/story/audit-records-run-end-and-limit.md (What changes, item 1)

story:exec-file-changes-are-recorded — it carries two deliverables with a seam the epic already cuts elsewhere: the host-side record on the in-process path, and the successor wire bundle (fact, route, event, refusals, daemon, store). The lead acceptance case names neither surface, so it can pass without the bundle. The sibling stories split the same way: `story:process-count-limit-hit-is-observed` is in-process and `story:exec-observation-reports-process-count-hits` is the wire. Fix: split the wire into a successor story that depends_on this one. That story also takes the bundle-order edges named in the closing line. — .engineering/planning/story/exec-file-changes-are-recorded.md:128-130, :136, :170

What I read: 2 drafts, plus 5 stories they build on or sit beside (`story:process-count-limit-hit-is-observed`, `story:audit-records-run-end-and-limit`, `story:run-observation-names-the-limit-that-ended-it`, `story:exec-start-carries-executable-lists`, `story:exec-observation-reports-process-count-hits`). Commands: `aep plan artifact show`, `relations`, `graph`, `validate` (reports `valid`) and `list --format json`. I walked 14 declared edges, outside the set down to `story:linux-executable-allow-list`, and found no cycle. I traced 4 acceptance reads to a producer: the usage story's limit-plus-usage on `run_ended`, which reads the limit and audit stories' outputs, and the file-change story's audit-sink case, which reads the audit story's sink write. Both reach their producers only through the transitive `depends_on` chain via `story:process-count-limit-hit-is-observed`, so no direct edge records them, which is acceptable. The file-change to usage edge is a shared-file edge (`mod.rs`) with its reason written beside it, so I raise no chain finding.

What I could not establish:
- Out of my lane, for parallel-safety: `story:exec-file-changes-are-recorded` cuts a bundle "at the next free version when it starts" and has no edge to `story:exec-start-carries-executable-lists` or `story:exec-observation-reports-process-count-hits`. Those two order their bundles explicitly and edit the same files: `crates/substrate-wire/src/lib.rs`, `scripts/gate.sh` and `runtime_vectors.rs`.
- Whether the cost of tree digests is acceptable is the ADR's to decide and is not judged here. I did not read the Loom or `sdc` sources the stories cite as origin.
- The cited lines support the claims I checked: `mod.rs:280-324` and `:696-706`, `lib.rs:2161-2185`, `:2308-2318` and `:1512`, `operations.rs:854-855`, `linux.rs:427` and `:507-526`, `process.rs:3115-3128`, and `systemd.rs:837-845`. No other citation mismatched.

```findings
- file: .engineering/planning/story/run-observation-carries-usage.md
  line: 76
  category: design
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the body never says how usage reaches the adapter, so two carriers now exist for the same counters: the host-internal end-of-run member that story:run-observation-names-the-limit-that-ended-it and story:process-count-limit-hit-is-observed own (OOM count, pids.events max, read for every exec measured or not) and ExecObservation.resource.usage, which a measured start fills with ResourceUsage carrying the same memory_oom_kills and process_limit_hits; the body must state whether usage extends that member or the linux profile asks for measurements, and which one the member then replaces, adding process.rs and observation.rs to scope if the former"
- file: .engineering/planning/story/exec-file-changes-are-recorded.md
  line: 128
  category: design
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "step 3 has the audit sink receive 'a record' but no ADR decision says whether that is a member of the run_ended record that docs/design/36-limit-hits.md and story:audit-records-run-end-and-limit own or a new AuditRecord variant, so two documents would own the sink contract; add the sink record's shape and size bound to the ADR's decision list"
- file: .engineering/planning/story/exec-file-changes-are-recorded.md
  line: 130
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the story carries two deliverables with a seam the epic already cuts elsewhere, the host-side record on the in-process path and the successor wire bundle (fact, route, event, refusals, daemon, store), and its lead acceptance case names neither surface; split the wire into a successor story that depends_on this one, as story:exec-observation-reports-process-count-hits does for story:process-count-limit-hit-is-observed, and let that story take the bundle-order edges"
```
