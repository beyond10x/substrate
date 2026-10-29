---
format: aep.planning-md/3
id: review-result:design-34-u5-security-pass-1
kind: review-result
status: active
title: Security review of design 34 U5, the namespace lane, pass 1 (4 warnings, 2 notes)
relations:
- reviews: story:kubernetes-namespace-workspace-and-exec-slice
revision: 1
---
# Security review of design 34 U5, the namespace lane, pass 1

Reviewed `wave/k8s-namespace-1` at `7b93629e` (U5 unit commit `425ab9c2`). Red cases: `55587341` on
`sec/u5-namespace-lane` (`xtask/tests/namespace_lane_security_review_u5.rs`, +241, test file only).
Cases 13 before, 17 after, 2 red.

Verdict: NEEDS-CHANGE. No blocker, 4 warnings, 2 notes.

## Findings

```findings
- {file: xtask/src/namespace_lane.rs, line: 683, category: integrity, severity: warning, verdict: NEEDS-CHANGE, origin: introduced, message: "a given kubeconfig whose loopback server carries tls-server-name, the mark of a tunnel to another cluster, passes the kind-only check (red: a_loopback_server_reached_through_a_tls_server_name_is_refused)"}
- {file: xtask/src/namespace_lane.rs, line: 678, category: boundary, severity: warning, verdict: NEEDS-CHANGE, origin: introduced, message: "the default kubeconfig named by KUBECONFIG is read when passed as --admin-kubeconfig, and the ~/.kube refusal is skipped when HOME is unset (red: the_file_the_ambient_kubeconfig_names_is_refused_when_given)"}
- {file: xtask/src/namespace_lane.rs, line: 569, category: judgement, severity: warning, verdict: CONFIRMED, origin: introduced, message: "given mode cannot tell a lane cluster from any other loopback kind cluster, so it provisions there"}
- {file: xtask/src/namespace_lane_observe.rs, line: 461, category: judgement, severity: warning, verdict: NEEDS-CHANGE, origin: introduced, message: "O8 reports held when the rules review is incomplete, which proves nothing about the rule set"}
- {file: xtask/src/namespace_lane.rs, line: 113, category: judgement, severity: note, verdict: CONFIRMED, origin: introduced, message: "a signal after the last observation leaves a printed Namespace-lane record beside a no-record message and exit 1"}
- {file: xtask/src/namespace_lane.rs, line: 635, category: contract-drift, severity: note, verdict: CONFIRMED, origin: introduced, message: "a failed cluster or namespace removal does not withhold the record, unlike the Chart-lane rule"}
```

## Cleared

- kind create, delete and nodes always name the run's own cluster and kubeconfig; the delete guard
  is armed only after create succeeds; nothing reads `KUBECONFIG` or `~/.kube` by itself.
- The record needs each of the 8 observations once, a clean checkout before and after and an
  unmoved HEAD; one line, three fields, `YYYY-MM-DDTHH:MM:SSZ`.
- No secret in the two observe logs or any error path; the bearer token is redacted.
- O3's control Pod turns an inconclusive run into an error, never held.
- The O8 tolerated default grants (`system:cluster-trust-bundle-discovery`,
  `system:service-account-issuer-discovery`) are v1.37.0 built-in grants to
  `system:serviceaccounts`; they grant the driver nothing.
- The CI job runs `check` only, with no cluster.

## Not verified

The CI job's duration, the release-side record parser (U12), anything at runtime.

## Disposition

All six go to fix unit I. Coordinator decision on finding 3: given mode refuses any cluster that is
not one this lane created: the kubeconfig's cluster and context must be
`kind-substrate-namespace-lane-*`, and before any write the API must list the matching
`substrate-namespace-lane-*-control-plane` node; anything else is refused before provisioning.
