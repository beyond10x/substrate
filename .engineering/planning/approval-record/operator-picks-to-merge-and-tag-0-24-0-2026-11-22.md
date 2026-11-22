---
format: aep.planning-md/3
id: approval-record:operator-picks-to-merge-and-tag-0-24-0-2026-11-22
kind: approval-record
status: draft
title: Operator's pick to merge the release change and tag 0.24.0 (2026-11-22)
revision: 1
---
# Operator's pick to merge the release change and tag 0.24.0 (2026-11-22)

## Who decided

The operator, on 2026-11-22 at 13:18 CET. His pick reached this repository's session as a relay
by session `host-operator`; it was not typed in this session, and it is not a decision of that
session.

## The words, as relayed

> host-operator relays Timo's pick, 2026-11-22 13:18 CET, to D16 "merge release PR 258 and tag
> 0.24.0?": "A: merge + tag when main green". Merge head dae5c8b2e with the bot; once gate.yml,
> chart-lane.yml, namespace-lane.yml and release-lanes.yml succeed on main for the merge commit,
> create and push the annotated bot tag 0.24.0 there; release.yml publishes. A red lane on main:
> report it, change no test. Back to me: tag object, release URL, run.

## What was done

- Pull request 258 was merged by the bot at `dae5c8b2e`, 21 of 21 checks green, as
  `85da2e2be46b690993007990905d6d3934650774`.
- `Gate`, `Chart lane`, `Namespace lane` and `Release lanes` each concluded `success` on `main`
  for that commit on their first run. No test was changed and no job was rerun on `main`.
- The annotated tag `0.24.0`, object `e92f67687f00cc6c5b6b0a96c61a6cab088085b1`, was created by
  the bot at that commit and pushed.
- Release run 45025006989 concluded `success`. The GitHub release was published at
  2026-11-22T13:46:55Z: <https://github.com/beyond10x/substrate/releases/tag/0.24.0>.
- Not verified from a workstation: pulling the images and their signatures. The workflow
  verifies each signature before it announces.

## What the release carries of the operator's security review

Fixed: finding 2, and finding 1 as a containment. The other findings are stories. The release
also carries the floor's refusal of the block-exchange requests, found beside the review, and
`url` 2.5.9 for RUSTSEC-2026-0592, which entered the advisory database on the day of the release.
