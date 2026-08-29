---
format: aep.planning-md/1
id: story:status-md-re-observed
kind: story
status: implemented
title: STATUS.md is observed again from the standalone repository
summary: STATUS.md is observed 2026-08-17, predates extraction, and still names the monorepo as the source.
owner: substrate
tags:
- docs
relations:
- decomposes: epic:release-hardening
- depends_on: story:ci-runs-the-full-gate
- depends_on: story:pinned-rust-toolchain
revision: 5
---
# Story: STATUS.md is observed again from the standalone repository

## Outcome

`STATUS.md` describes the repository a reader is standing in: the standalone public repository at
tag `0.2.1`, with a *Release* row and a *CI* row, every count taken from that day's gate output.

## Context

`STATUS.md:3` is observed 2026-08-17, before the 2026-08-23 extraction; its *Source* row
(`STATUS.md:17`) still says `foundation/substrate in the predecessor monorepo`. `AGENTS.md`
§ *Document placement* makes observed progress `STATUS.md`'s job; a stale one is a placement
violation. It depends on `story:ci-runs-the-full-gate` and `story:pinned-rust-toolchain` so the
CI row is not stale the same week.

## Acceptance

`STATUS.md` names the standalone repository and tag `0.2.1`, carries *Release* and *CI* rows, and
every count in it is copied from a command whose output is recorded in this story on the
observation date.

Evidence that satisfies it:

- `**Observed:**` is the refresh date;
- the *Wire contract* counts (operations, vectors, requirements, hash fixtures) come from
  `scripts/check-contract-bundle-0.4.0.py` output pasted here;
- the *Release* row: `Dockerfile` present, no publish workflow, no signing, no digest pin — next
  proof `story:signed-daemon-image`;
- `## Repository facts` contains no sentence true only of the monorepo era, and each remaining
  fact names the test or script that proves it;
- `bash scripts/gate.sh` exits 0.
