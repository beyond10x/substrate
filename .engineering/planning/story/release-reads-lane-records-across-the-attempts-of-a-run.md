---
format: aep.planning-md/3
id: story:release-reads-lane-records-across-the-attempts-of-a-run
kind: story
status: draft
title: The release reads each lane's record from the attempt that made it
tags:
- release
scope:
- confidence: cited
  path: .github/workflows/release.yml
- confidence: cited
  path: xtask/tests/release_workflow.rs
revision: 2
---
# Story: the release reads each lane's record from the attempt that made it

## Outcome

`release.yml` finds all four hosted lane records of a `release-lanes.yml` run whose failed job
was run again, without a recovery dispatch.

## What was seen

2026-11-23, release 0.26.0, tagged commit `d0bd8be`: the hosted run 45161004811 failed in the
confinement lane on its first attempt; the other three lanes succeeded and annotated their
records. A rerun of the failed job alone made attempt 2 succeed. The tag-push release run
45169990519 was refused in its preflight: `refused: no recorded quota lane run`. The step
*Read the hosted lane records for this commit* lists the jobs of the run's latest attempt
(`actions/runs/<id>/jobs`), and the three jobs carried over into attempt 2 have new check runs
without annotations. The release was published by the documented recovery dispatch, run
45170446961, with the three records of attempt 1 as inputs.

## What changes

The step reads the jobs of every attempt of the chosen run (`filter=all`, or
`attempts/<n>/jobs`) and takes, per lane, the annotation of the newest attempt in which that
lane's job succeeded. Two annotations of one lane from one attempt stay a refusal.

## Design

No change to what a record is or to which runs count (a `push` on `main` or a
`workflow_dispatch` of this repository). `AGENTS.md` § Releases, *The hosted lane records*,
states the rule.

## Acceptance

`xtask/tests/release_workflow.rs`: a recorded two-attempt run, with one lane annotated only in
attempt 2 and three only in attempt 1, yields all four records; a lane whose job never
succeeded is still refused. Offline, under 5 s.

