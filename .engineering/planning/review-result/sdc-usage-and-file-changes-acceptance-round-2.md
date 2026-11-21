---
format: aep.planning-md/3
id: review-result:sdc-usage-and-file-changes-acceptance-round-2
kind: review-result
status: active
title: 'Plan critic (acceptance), round 2: usage on the observation and the file-change record'
tags:
- plan-critic
- sdc
relations:
- reviews: story:run-observation-carries-usage
- reviews: story:exec-file-changes-are-recorded
- reviews: story:exec-file-change-record-on-the-wire
revision: 1
---
needs-revision

story:exec-file-changes-are-recorded — the check "A profile that does not ask for the record runs as before, with no digest taken" names no observable for "no digest taken", and the Outcome's "every run on the `linux` adapter has a change record" contradicts it, so the story never states what the adapter API returns for a run that did not ask — .engineering/planning/story/exec-file-changes-are-recorded.md:151
story:exec-file-changes-are-recorded — the story adds a fact the adapter proves and limits itself to `linux` (and `systemd` around `linux`), but no check says what `local`, `macos`, `oci` or `pod`, or a `linux` profile that asks for the record without the fact, return, so invariant 3's absent-or-named-refusal is unchecked (the sibling story checks this at run-observation-carries-usage.md:109) — .engineering/planning/story/exec-file-changes-are-recorded.md:162
story:exec-file-change-record-on-the-wire — the acceptance sentence joins two independent outcomes with "and": "the event for its record arrives on the event stream, and the route returns the record …", so the event can fail while the route passes; the transition clause (the route absent on the predecessor bundle) is a third statement — .engineering/planning/story/exec-file-change-record-on-the-wire.md:90

What I read: 3 of 3 ids (`story:run-observation-carries-usage`, `story:exec-file-changes-are-recorded`, `story:exec-file-change-record-on-the-wire`), each whole through `aep plan artifact show <id>`. I also ran `grep -n` over the three files for acceptance line numbers and `git grep` for an existing no-digest seam. I read the rubric and the `plan-critic-acceptance` procedure.

I judged `story:run-observation-carries-usage` observable, transitional (before: no usage field) and checkable by its delegated-lane case, so it has no finding. I did not reopen the § Decided items of `story:exec-file-changes-are-recorded`.

What I could not establish:
- Whether the ADR will report a rename as a `rename` op or as delete plus create. The acceptance at exec-file-changes-are-recorded.md:130 requires a rename to be listed, and tree digests alone do not make a rename obvious. This is a design question, so I did not count it as a finding; the ADR must settle it before the acceptance can pass.
- Out of my lane: the `depends_on story:exec-observation-reports-process-count-hits` edge on the wire story points at a story that is not among the three I was given, and the ordering and shared-surface claims in the three `## Order` sections belong to the design and parallel-safety critics.
- In `story:run-observation-carries-usage`, "A figure the host could not read is absent" (What changes 2) has no matching check. I judged this too thin to be a finding.

```findings
- file: .engineering/planning/story/exec-file-changes-are-recorded.md
  line: 151
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the check 'A profile that does not ask for the record runs as before, with no digest taken' names no observable for 'no digest taken', and the Outcome's 'every run on the linux adapter has a change record' contradicts it, so the story never states what the adapter API returns for a run that did not ask"
- file: .engineering/planning/story/exec-file-changes-are-recorded.md
  line: 162
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the story adds a fact the adapter proves and limits itself to linux (and systemd around linux), but no check says what local, macos, oci or pod, or a linux profile that asks for the record without the fact, return, so invariant 3's absent-or-named-refusal is unchecked"
- file: .engineering/planning/story/exec-file-change-record-on-the-wire.md
  line: 90
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the acceptance sentence joins two independent outcomes with 'and' (the event arrives on the event stream, and the route returns the record) plus a third clause that the route is absent on the predecessor bundle, so one can pass while another fails"
```
