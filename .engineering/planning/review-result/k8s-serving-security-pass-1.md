---
format: aep.planning-md/3
id: review-result:k8s-serving-security-pass-1
kind: review-result
status: active
title: 'Security review of the node-bound Kubernetes serving chart, pass 1 (needs change: 9 findings)'
relations:
- reviews: story:node-bound-kubernetes-serving-profile
revision: 1
---
unit: uncommitted working tree of story/node-bound-kubernetes-serving-profile on 1be2529c (chart, chart-lane.yml, release.yml, xtask chart*.rs), 2026-10-28
verdict: NEEDS-CHANGE
cases: executed 381→386, red 6 (5 new, 1 existing turned red by an added refusal row)
origin: introduced 9 / pre-existing 0 / undecided 0
needs-coordinator: none

1. **Cases written:** two `chart.rs` unit cases, three static-lane refusal rows, two values injected with `--skip-schema-validation`, and three release-step cases in `xtask/tests/release_workflow.rs`; red against the unit as reviewed.
2. **Checked and cleared:** with the schema in force no privileged container, host namespace, hostPath or capability without `execution.enabled`; the schema is closed; key material is copied with mode 0400 into memory-backed emptyDirs and appears in no ConfigMap, env, args or NOTES; no development-TCP posture; NetworkPolicy covers both directions; port 9090 is reachable from scrapers only (ADR 0045 section 3); `attestations: write` on the release job only; actions pinned by SHA; chart-lane.yml uses `pull_request`, `contents: read`, no secrets.
3. No cluster was used: every kube context on the review machine is a production cluster.

```findings
[{"file":"deploy/helm/substrate/templates/statefulset.yaml","line":43,"category":"contract-drift","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"unquoted value interpolation lets --skip-schema-validation render hostPID/hostNetwork/hostIPC and a hostPath volume, contradicting the CHANGELOG claim that template checks refuse them"},
{"file":"deploy/helm/substrate/templates/_helpers.tpl","line":53,"category":"boundary","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"the any-address egress guard compares strings, so 10.0.0.0/0, 1.2.3.4/0, 0::/0 and ::0/0 pass schema, template and lane check without allowAnyEgress"},
{"file":".github/workflows/release.yml","line":1283,"category":"integrity","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"any oras resolve error is read as tag absent, so a registry error leads to helm push over an existing chart tag"},
{"file":".github/workflows/release.yml","line":1285,"category":"acceptance","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"an existing chart tag is refused only after the bundle copy and daemon and MCP pushes, not in the up-front write-once check"},
{"file":".github/workflows/release.yml","line":1288,"category":"integrity","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"a recovery signs and attests an existing chart after matching one values line read by tag, never comparing its templates to the tagged source"},
{"file":"xtask/src/chart.rs","line":650,"category":"mutant","severity":"note","verdict":"NEEDS-CHANGE","origin":"introduced","message":"pod_posture never checks readOnlyRootFilesystem, so removing it from the templates leaves the lane green"},
{"file":"deploy/helm/substrate/values.schema.json","line":283,"category":"boundary","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"podAnnotations admits an AppArmor-unconfined container annotation that applies to the execution-off pod, which sets no appArmorProfile"},
{"file":".github/workflows/release.yml","line":1265,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"helm registry login writes the GHCR token to disk before package-chart runs repository code, while the test message claims that step receives no token"},
{"file":"xtask/src/chart_cluster.rs","line":980,"category":"acceptance","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"no lane starts an execution-enabled pod and no K9 fallback-C record exists, so the execution profile is neither proven nor reported absent"}]
```
