---
format: aep.planning-md/3
id: approval-record:design-30-accepted
kind: approval-record
status: active
title: Operator accepts design 30 with every recommended answer (ADR 0048)
relations:
- decides: story:confinement-adapter-on-the-wire
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-10-31T11:27:17Z", actor: "human:timo", revision: 2}
---
# Decision: design 30 accepted with every recommended answer

Taken 2026-10-31 by the operator and relayed by the coordinating session (D326). The operator
answered `decision-blocker:namespace-exec-needs-design-30` through the session that acts on the
operator's behalf, 2026-10-31 10:0xZ, verbatim: "take decissions like the above for me (A)". The
operator confirmed the same day that this session acts on the operator's behalf and may approve
ADRs.

Option A was: accept design 30 with the recommended answer to each of D1 to D10 of its § 9 (D1 A
facts only, D2 one boolean per family, D3 new names, D4 daemon-checked `sandbox.requires`, D5
optional `cgroup` now, D6 never `local`, D7 operational listener only, D8 an ADR, D9 per entry-gate
story, D10 next free bundle), recorded in an ADR.

## What follows

- ADR 0048 records the answers; design 30 moves to `accepted`. Its bundle is `0.26.0` (D10).
- `story:confinement-adapter-on-the-wire` leaves `draft` and is implemented in wave
  `approval-record:wave-2026-10-31-design-30`.
- `story:namespace-driver-admits-an-exec-on-observed-confinement` is filed: the namespace probe
  publishes the four families, `confinement.whole-tree-kill` and `confinement.no-egress` from
  observation, and the namespace lane's exec cases run on a kind cluster whose network plugin
  enforces NetworkPolicy.
