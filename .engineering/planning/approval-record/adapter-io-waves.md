---
format: aep.planning-md/3
id: approval-record:adapter-io-waves
kind: approval-record
status: draft
title: Run the adapter standard-input and live-output waves and release after each (D166)
relations:
- decides: story:adapter-stdin-pipe
- decides: story:adapter-live-output
revision: 2
---
# Decision: run the adapter standard-input and live-output waves, and release after each

Taken 2026-10-28 by the coordinating session as its D166, under the operator's standing grant
(class C: a wave), relayed to this repository's session.

- After design 32 merges, `story:adapter-stdin-pipe` moves to active and runs one wave: a spec
  author writes its first commit, an implementor the adapter commits, an adversary attacks the
  result; it merges with every lane green.
- `story:adapter-live-output` follows the same way, after `story:adapter-stdin-pipe`.
- Substrate is released through its own release process after each story merges, because a
  consumer waits on both.
- The ESS 0.75.0 pin stays last in the open pull request order; it may merge before the
  standard-input wave if it is ready earlier.

## Amended by D169 (2026-10-28)

`approval-record:one-pr-per-wave`: both stories merge into the integration branch
`wave/adapter-io`, which reaches `main` in one pull request and is released once, after both. The
release-after-each line above no longer holds.
