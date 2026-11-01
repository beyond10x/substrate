---
format: aep.planning-md/3
id: story:namespace-lane-same-named-case-creates-its-workspace
kind: story
status: proposed
title: The namespace lane's same-named-object case creates its workspace on every run
relations:
- informed_by: review-result:wave-2026-10-31-d337-adversary-pass-1
- informed_by: approval-record:d338-review-findings-decided-in-session
revision: 2
transitions:
- {from: "draft", to: "proposed", at: "2026-11-01T00:28:53Z", actor: "human:timo", revision: 2}
---
# Story: the namespace lane's same-named-object case creates its workspace on every run

## Outcome

Step 6 "same-named object of another deployment untouched" of `cargo xtask namespace-lane observe`
creates its workspace on every run. A create the driver answers 500 `operation.outcome-unknown`
there is either settled by the lane through `GET /v1/ops/{op}` or traced to its cause and fixed in
the driver, and step 7 no longer finds the case's keeper Pod and claim left behind.

## Evidence

- Failed once in three runs: the four-lane run at `620e89ce` (2026-10-31), step 6 answered 500
  `operation.outcome-unknown` on the workspace create, and step 7 then found that workspace's
  keeper Pod and claim 180 s after the daemon served again.
- Passed at `3b3998c8` (D332 trace run 1), in the D337 traced run at `620e89ce` and in the release
  lanes at `7b3cf9db` (namespace lane, 8 steps).
- Cause not observed: the failing run kept no daemon trace of the create.

## Design

First observe: run the step under the D337 trace (daemon log at debug for
`substrate_kubernetes::namespace`) until it fails once, and read which API call answered unknown.
Then either the driver settles that call (a retried read of the name it created) or the lane treats
a retriable `operation.outcome-unknown` as design 01 § 6 does and reads the operation to its end,
as the clean-room runner does since 0.17.0. Out-of-scope row of
`review-result:wave-2026-10-31-d337-adversary-pass-1`, adopted under D338.

## Acceptance

- A lane unit test where the create answers 500 `operation.outcome-unknown` once and the operation
  then reads `succeeded` passes step 6 and leaves nothing for step 7.
- If the trace shows a driver defect, a driver test reproduces that answer and passes after the fix.
