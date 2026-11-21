---
format: aep.planning-md/3
id: review-result:sdc-usage-and-file-changes-acceptance-round-1
kind: review-result
status: active
title: 'Plan critic (acceptance), round 1: usage on the observation and the file-change record'
tags:
- plan-critic
- sdc
relations:
- reviews: story:run-observation-carries-usage
- reviews: story:exec-file-changes-are-recorded
revision: 1
---
needs-revision
run-observation-carries-usage — the acceptance case checks only peak memory and CPU time, so no check shows that wall time and peak process count are carried: "peak memory is at least 64 MiB and whose CPU time is at least one second" — .engineering/planning/story/run-observation-carries-usage.md:87
run-observation-carries-usage — the checks give `oci` and `pod` no observable outcome, so the summary's "absent elsewhere" and the design row's "from what their runtime states or absent" can both be met by any result — .engineering/planning/story/run-observation-carries-usage.md:93
exec-file-changes-are-recorded — "A file an exec wrote and restored to its starting content yields no record" uses "no record" for a different thing than the overlap case does, and it does not say whether an exec with no net change gets an empty record or none, against the Outcome's "For every exec, Substrate records" — .engineering/planning/story/exec-file-changes-are-recorded.md:145
exec-file-changes-are-recorded — no check reads the record through the route or receives the event, though the Outcome promises both and the headline case does not say which surface it reads — .engineering/planning/story/exec-file-changes-are-recorded.md:136
exec-file-changes-are-recorded — the Outcome covers `/workspace`, declared writable roots and `/scratch`, but the acceptance exercises only "two workspaces", and nothing checks the cost bound above which a start is refused or the record is absent (§ What changes item 1) — .engineering/planning/story/exec-file-changes-are-recorded.md:136

What I read: 2 artifacts (`story:run-observation-carries-usage`, `story:exec-file-changes-are-recorded`) with `aep plan artifact show <id>`, the rubric and role file, and `RunObservation` at `crates/substrate-host/src/confinement/mod.rs:696` and `linux.rs:427` to confirm the cited facts.

What I could not establish:
- Overlap detection beyond two execs (a session, a file-route write) is deferred to the ADR, so I did not call its missing check a finding. The ADR's own checks will need it.
- Whether the `RunObservation` field (`run-observation-carries-usage`) and the new record (`exec-file-changes-are-recorded`) collide on `confinement/mod.rs` is parallel-safety, which is out of my lane.
- Whether `docs/design/36-limit-hits.md` exists yet is design and scope territory, which is out of my lane.

```findings
- file: .engineering/planning/story/run-observation-carries-usage.md
  line: 87
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the acceptance case checks only peak memory and CPU time, so no check shows that wall time and peak process count are carried"
- file: .engineering/planning/story/run-observation-carries-usage.md
  line: 93
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the checks give oci and pod no observable outcome, so the summary's 'absent elsewhere' and the design row's 'from what their runtime states or absent' can both be met by any result"
- file: .engineering/planning/story/exec-file-changes-are-recorded.md
  line: 145
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "'A file an exec wrote and restored to its starting content yields no record' uses 'no record' for a different thing than the overlap case does, and does not say whether an exec with no net change gets an empty record or none, against the Outcome's 'For every exec, Substrate records'"
- file: .engineering/planning/story/exec-file-changes-are-recorded.md
  line: 136
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "no check reads the record through the route or receives the event, though the Outcome promises both and the headline case does not say which surface it reads"
- file: .engineering/planning/story/exec-file-changes-are-recorded.md
  line: 136
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the Outcome covers /workspace, declared writable roots and /scratch, but the acceptance exercises only two workspaces, and nothing checks the cost bound above which a start is refused or the record is absent"
```
