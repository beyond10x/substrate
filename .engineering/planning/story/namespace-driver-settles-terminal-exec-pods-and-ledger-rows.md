---
format: aep.planning-md/3
id: story:namespace-driver-settles-terminal-exec-pods-and-ledger-rows
kind: story
status: draft
title: The namespace driver settles terminal exec Pods and their ledger rows
summary: A scheduled bounded round removes ended exec Pods, retries failed deletions and settles the rows, across restarts
tags:
- defect
revision: 1
---
# Story: the namespace driver settles terminal exec Pods and their ledger rows

## Outcome

After an exec ended and was acknowledged, its Pod is deleted, a failed deletion is retried, and
its ledger row is settled, with the daemon running or after a restart. The driver's memory and
the rows a probe round loads return to their baseline.

## What changes

1. The cleanup round runs in the daemon's lifecycle; design 34 names the timer as missing.
2. The round settles what a restart left: it reads the unsettled rows and not only the names
   held in memory.
3. The round is bounded per pass and keeps a name that was added while it ran.

## Design

Design 34 gets the round's schedule and bounds. No wire schema changes.

## Acceptance

Cases on the crate's fake cluster: after successful ends the set and the unsettled rows are at
baseline; a deletion that fails once is retried and settled; a restart settles what it finds.


## Where the requirement comes from

The operator's private security review of `main` at `1af046902`, dated 2026-11-22,
finding 8. The operator ordered its findings handled on 2026-11-22. By
`approval-record:operator-decides-how-security-review-fixes-travel-2026-11-22` this story
states the rule that must hold; the review's detail stays with the operator until the fix is
on `main`. Checked against the source at `1af046902` by reading; nothing was run. Release
0.23.0 has the same source. Stakeholder: the operator.
