---
format: aep.planning-md/3
id: approval-record:d333-timeout-never-proves-no-egress
kind: approval-record
status: active
title: 'D333: a timed-out attempt never proves confinement.no-egress'
relations:
- decides: story:namespace-probe-never-proves-no-egress-from-a-timeout
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-10-31T18:33:18Z", actor: "human:timo", revision: 2}
---
# D333: a timed-out attempt never proves confinement.no-egress

## Approval

b10x-chef-orchestrator, 2026-10-31, D333, choosing option C of the three fail-closed options the
D332 traces raised: "Ship A in 0.18.0: a timeout never proves no-egress; only a local deny
(EPERM/EACCES, ENETUNREACH/EHOSTUNREACH) does. Failing test first, reproducing both traced runs.
File B as a proposed story citing both traces; its design 34 amendment is decided when that story
is picked, not now." The same wave corrects README, STATUS, ROADMAP, the architecture overview, the
website pages and the CHANGELOG `[0.18.0]` section.

## Options considered

- A: only a local deny proves an attempt blocked; a timeout proves nothing. Exec stays refused on
  plugins that drop packets silently, as in 0.17.0.
- B: several probe Pods over time, each attempting at its first instant; publish only when every
  one is blocked, and withhold for the binding generation once any attempt is reached or refused.
- C: A now, B as a designed story. Chosen.

## Consequence

A namespace driver admits an exec only on a cluster whose network plugin denies the probe's three
attempts locally (`EPERM`, `EACCES`, `ENETUNREACH`, `EHOSTUNREACH`). Plugins that drop packets
silently, kind's among them, leave `confinement.no-egress` absent and every exec start refused
`exec.sandbox-unavailable`, as in 0.17.0.
