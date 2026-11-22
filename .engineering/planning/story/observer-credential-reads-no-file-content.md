---
format: aep.planning-md/3
id: story:observer-credential-reads-no-file-content
kind: story
status: draft
title: A credential narrowed to observation reads no workspace file content
summary: Operation observations and events carry no file text for a scope that cannot read the workspace
tags:
- security
revision: 1
---
# Story: a credential narrowed to observation reads no workspace file content

## Outcome

Under the hosted admission profile (ADR 0026), a credential that holds only the observation
scope reads the state and the metrics of operations and no text of a workspace file, for
retained records as for new ones. A credential that may read the workspace keeps the full result
of its file changes.

## Design

Owed before code: an ADR that amends ADR 0026. No accepted text says what an observer may read
of a result. The ADR decides between a content-free projection for the observation routes and a
result that holds no file text at all, and says what happens to retained records. A change of
what a route answers per scope adds a successor bundle.

## Acceptance

With an observation-only credential, the operation routes and the event list and stream answer
without file text; with a workspace credential the mutation result is unchanged. Held by a
hosted-admission case.


## Where the requirement comes from

The operator's private security review of `main` at `1af046902`, dated 2026-11-22,
finding 6. The operator ordered its findings handled on 2026-11-22. By
`approval-record:operator-decides-how-security-review-fixes-travel-2026-11-22` this story
states the rule that must hold; the review's detail stays with the operator until the fix is
on `main`. Checked against the source at `1af046902` by reading; nothing was run. Release
0.23.0 has the same source. Stakeholder: the operator.
