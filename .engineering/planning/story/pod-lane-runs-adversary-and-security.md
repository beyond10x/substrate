---
format: aep.planning-md/3
id: story:pod-lane-runs-adversary-and-security
kind: story
status: implemented
title: The CI pod lane runs the pod adversary and security cases
tags:
- adr-0109
relations:
- decomposes: epic:confinement-contract-family
- depends_on: story:pod-adapter
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-27T07:24:38Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-27T07:24:38Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-27T13:17:26Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":2}}}
---
## Outcome

The gate workflow's `pod-lane` job runs the pod adversary and security-review cases
(`crates/substrate-host/tests/adversary_pod.rs`, `security_review_pod.rs`) against its pinned
`kind` cluster, beside `ess_confinement_pod`, and fails when any of them is absent or fails.

## Origin

`story:pod-adapter`: those cases ran only against a local kind cluster; CI runs only the ESS pod
scenarios (PR 160, "Not in this change"; `review-result:adr-0109-u7-fix-decisions`).

## Acceptance

A `pod-lane` run on the pull request prints each suite's own `test result:` line with a non-zero
passed count for all three suites, and a run where one of them selects no case is red.
