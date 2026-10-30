---
format: aep.planning-md/3
id: story:exec-end-after-await-never-panics
kind: story
status: draft
title: An exec end decided after an await never panics on a concurrent end
summary: end() after a GET await can reach unreachable! when a concurrent observation ended and acknowledged the exec
relations:
- decomposes: epic:kubernetes-deployment-and-driver
scope:
- confidence: cited
  path: crates/substrate-kubernetes/src/exec.rs
revision: 2
---
# Story: An exec end decided after an await never panics on a concurrent end

## Outcome

The namespace driver's exec observation decides an end (finished, gone, Unknown) from a snapshot
taken before a GET, then calls `end` after the await. If a concurrent observation ends the exec and
the daemon acknowledges it in between, `end` reaches `unreachable!`. After this story the late
caller finds the exec already ended and returns that end, never a panic.

## Finding

Adversary pass 1 on `story:unconfirmed-pod-deletion-ends-unknown`
(`review-result:wave-2026-10-30-u4-adversary-pass-1`) read the window in
`crates/substrate-kubernetes/src/exec.rs` (`end`, near line 912). The `finish` and `gone` branches
share it; it predates that story. The daemon's `put_exec` answers `Superseded`, so the persisted
state is safe, but the panic takes the observing task down.

## Acceptance

- A red case first: two observations of one exec race so that one ends and acknowledges it while
  the other awaits its GET; today the second panics.
- After the change, the second returns the recorded end; no `unreachable!` remains on that path.

## Design

Design 34 § 7. No contract change.
