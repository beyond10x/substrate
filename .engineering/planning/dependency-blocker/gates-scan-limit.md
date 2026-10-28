---
format: aep.planning-md/3
id: dependency-blocker:gates-scan-limit
kind: dependency-blocker
status: open
title: 'The org source gate refuses substrate main: candidate exceeds scan limit'
relations:
- blocks: story:pod-unknown-cleanup-outlives-adapter
- blocks: story:node-bound-kubernetes-serving-profile
revision: 1
---
# The org source gate refuses substrate main: candidate exceeds scan limit

## Observation

From merge `cbf4fd88` (PR 178) on, the required check `common / Security and privacy` fails on
main and on every pull request with `b10x-gates: candidate exceeds scan limit`:

- main push run 40705855610 (2026-10-28T14:25Z);
- PR 181 run 40706506289 (2026-10-28T14:29Z).

The last green main push is run 40697691904 (2026-10-28T13:34Z) at `5b9daca9`.

## Cause

`b10x-gates` (`gates/src/git.rs`, `candidate`) sums the changed files of every commit from the
repository's adoption baseline in the org policy to the candidate head, and refuses above
`MAX_TOTAL` = 512 MiB. Substrate's baseline was `473091a9` (2026-09-15); 537 commits are on main
since it. Inferred, not measured: bundle `0.24.0` in PR 178 carried the sum over the limit.

## Decision D144 (orchestrator, 2026-10-28T14:45Z)

Substrate's adoption baseline moves to `5b9daca9`, the last green main push. The org policy commit
and the Actions secret refresh are made outside this repository by the collection's policy owner.
This repository changes nothing for it.

## Clears when

`common / Security and privacy` is green on main again. Pull requests 181, 182, 183 and 184 merge
after that.
