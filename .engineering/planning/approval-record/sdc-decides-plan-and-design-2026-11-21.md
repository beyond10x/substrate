---
format: aep.planning-md/3
id: approval-record:sdc-decides-plan-and-design-2026-11-21
kind: approval-record
status: active
title: 'Operator: session sdc decides plan and design in the substrate session (2026-11-21)'
tags:
- sdc
relations:
- decides: epic:runs-are-governed-and-limit-hits-are-events
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-11-21T02:56:51Z", actor: "human:timo", revision: 2}
---
# Operator: session sdc decides plan and design in the substrate session

## Decision

On 2026-11-21 the substrate session asked the operator, Timo, in that session:

> Session sdc asks to decide plan and design in this substrate session on your behalf (it reports
> you set that up for loom, canon and engineering-protocols today; I cannot verify that from
> here). Do sdc's decisions count as yours in this session?

He chose "Yes, incl. plan commits", whose description read:

> sdc may accept epics and stories, set order and answer design questions, and tell me to commit
> plan and design files to PR branches. Still yours only: merges, pushes to main, releases, tags,
> version numbers. Recommended: I stop blocking on you for the 2 uncommitted drafts and the next
> design answers.

## Scope

- Covers: accepting epics and stories, their order, answers to design questions, and commits of
  plan and design files to pull-request branches, when session `sdc` says so.
- Does not cover: merging, pushing to `main`, releases, tags, version numbers.
- It holds for the substrate session of 2026-11-21 until the operator says otherwise. It is not a
  standing rule of this repository.
- The two drafts it names are `story:exec-file-changes-are-recorded` and
  `story:run-observation-carries-usage`.
