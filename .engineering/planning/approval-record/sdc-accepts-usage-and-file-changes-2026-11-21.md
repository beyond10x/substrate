---
format: aep.planning-md/3
id: approval-record:sdc-accepts-usage-and-file-changes-2026-11-21
kind: approval-record
status: active
title: Session sdc accepts the usage story and the file-change record story (2026-11-21)
tags:
- sdc
relations:
- decides: story:run-observation-carries-usage
- decides: story:exec-file-changes-are-recorded
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-11-21T03:08:31Z", actor: "human:timo", revision: 2}
---
# Session sdc accepts the usage story and the file-change record story

## Decision

Session `sdc` wrote to the substrate session on 2026-11-21, with the authority
`approval-record:sdc-decides-plan-and-design-2026-11-21` records:

> Decisions: accept story:run-observation-carries-usage and story:exec-file-changes-are-recorded
> (both to your accepted state once your critic panel and scoping are done; requests from the sdc
> session are accepted by default from now on, per the operator's standing rule of today). Order:
> both after story:process-count-limit-hit-is-observed, usage first (it is small and Loom's
> adapter-route story and sdc's KPIs read it), then the file-change record with its ADR and ESS
> model before code, mechanism: pre and post tree digests per exec, one writer per workspace, no
> fact published when a second exec wrote the same workspace during the exec, no time-window
> fallback.

## What was done on it

- Both stories have typed scope and went through two rounds of three plan critics (acceptance,
  design, parallel safety). The scope critic did not run: neither story decomposes a parent.
- Round 1 returned 14 findings and round 2 returned 7; all 21 are recorded as `fixed`. The fixes
  made after round 2 were not read by a critic again.
- `story:run-observation-carries-usage` and `story:exec-file-changes-are-recorded` move to
  active.

## What it does not cover

- `story:exec-file-change-record-on-the-wire` was split out of the file-change story after the
  design critic's round 1. Session `sdc` has not seen it as a separate story; it stays draft.
- "requests from the sdc session are accepted by default from now on" is session `sdc`'s
  statement of the operator's rule. In the substrate session the operator's own answer covers
  plan and design decisions and plan commits; merges, pushes to `main`, releases, tags and version
  numbers stay his.
