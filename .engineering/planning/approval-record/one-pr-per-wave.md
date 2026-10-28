---
format: aep.planning-md/3
id: approval-record:one-pr-per-wave
kind: approval-record
status: draft
title: One pull request to main per wave (D169)
relations:
- decides: story:adapter-stdin-pipe
- decides: story:adapter-live-output
revision: 1
---
# Decision: one pull request to main per wave

Taken 2026-10-28 by the coordinating session as its D169, under the operator's standing grant,
relayed to this repository's session.

## Why

On 2026-10-28 this repository merged nine separate pull requests to `main` (181, 183 to 189, 191).
Each paid every required and path-triggered workflow again: the full gate, the chart lane, the
macOS host crate, documentation and the shared source gates.

## Rule

- Each unit branch of a wave merges into the wave's integration branch, as the wave protocol
  describes; it does not open its own pull request to `main`.
- Planning-store hygiene, pins and small fixes ride along in the same integration branch.
- One pull request goes from the integration branch to `main` once the wave's gate is green, and
  one release follows.
- Exception: a fix a consumer is blocked on today may go alone; its pull request body names the
  blocked consumer.

## Applied now

- `story:adapter-stdin-pipe` and `story:adapter-live-output` share the integration branch
  `wave/adapter-io`, one pull request and one release. The consumer waits on both, so one release
  serves it. This replaces the release-after-each line of `approval-record:adapter-io-waves`.
- Pull request 192 (gate wall time) was opened and its checks started before this decision; it
  merges alone because its run is already paid and it shortens every later gate run.
- The planning-hygiene branch `store/hygiene-2026-10-28c` rides in `wave/adapter-io`.
