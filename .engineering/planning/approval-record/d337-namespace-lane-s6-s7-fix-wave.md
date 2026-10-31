---
format: aep.planning-md/3
id: approval-record:d337-namespace-lane-s6-s7-fix-wave
kind: approval-record
status: active
title: 'D337: fix the namespace lane''s S6 and S7 failures before the 0.18.0 tag'
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-10-31T20:53:48Z", actor: "human:timo", revision: 2}
---
# D337: fix the namespace lane's S6 and S7 failures before the 0.18.0 tag

## Approval

b10x-chef-orchestrator, 2026-10-31, D337: "(1) one traced namespace-lane run at 620e89ce that
captures the daemon log and the workspace-create request/response for S6 "same-named object of
another deployment", to settle the 500 operation.outcome-unknown as flake or defect by observation
… (2) Fix wave on a new integration branch: S6a, the probe logs a reason code
(kubernetes.api-failed or "<fact> was not proved") when the pod-log read is refused, failing test
first; S6b if (1) shows a defect, failing test first; S7 only if it survives S6b. (3) adversary
pass, local gate (slot request), bot PR, merge on green, all four lanes at the new main, tag.
Stop criterion unchanged."

## What it answers

The namespace lane at `620e89ce` (0.18.0 candidate, after D333 and D334) passed S3 and S4 and
failed three steps:

- S6 "Role without pods/log get": nothing refused within 120 s, no reason code in the log. The
  confinement facts were withheld, but the lane could not attribute their absence. The same
  failure occurred at `3b3998c8` (D332 trace run 1).
- S6 "same-named object of another deployment untouched": creating the workspace answered 500
  `operation.outcome-unknown`; it passed at `3b3998c8`.
- S7: that workspace's keeper Pod and claim were left 180 s after the daemon served again.

The other three release lanes passed at `620e89ce`: Confinement 189, Quota 7, Chart 4.
