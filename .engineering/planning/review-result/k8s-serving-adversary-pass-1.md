---
format: aep.planning-md/3
id: review-result:k8s-serving-adversary-pass-1
kind: review-result
status: active
title: 'Adversary pass on the node-bound Kubernetes serving chart, pass 1 (needs change: 10 red cases)'
relations:
- reviews: story:node-bound-kubernetes-serving-profile
revision: 1
---
unit: uncommitted working tree of story/node-bound-kubernetes-serving-profile on 1be2529c (chart, chart-lane.yml, release.yml, xtask chart*.rs), 2026-10-28
verdict: NEEDS-CHANGE
cases: executed 65→76, red 10
origin: introduced 9 / pre-existing 1 / undecided 0
needs-coordinator: paste the cases (the reviewer did not edit the unit's tree)

1. **Cases written:** `xtask/src/chart_adversary.rs` (8 cases, static lane and YAML reader) and three release preflight cases appended to `xtask/tests/release_workflow.rs`; all red against the unit as reviewed.
2. **Attacked and not broken:** multi-document splitting, quoted keys, duplicate keys, inline comments, value-position anchors and tags; the 26 schema refusals; headless Service, probe ports, state path, fsGroup, PVC retention, init-container modes; certificate names; refusal of a cancelled or failed chart-lane run; packaging pins the daemon digest once.
3. **Judgement findings** J1 to J3 have no case: they need a cluster or concern CI triggers.

```findings
[{"file":"xtask/src/chart.rs","line":961,"category":"boundary","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"Any-address egress spelled 10.0.0.0/0, 0::/0 or ::0/0 passes the schema, the template guard and the static lane without allowAnyEgress, because both compare two exact strings."},
{"file":"xtask/src/chart.rs","line":950,"category":"mutant","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"The lane accepts NetworkPolicy peers with empty selectors and port entries without a port, so a template that opens ingress or egress to every pod or port stays green."},
{"file":"xtask/src/chart.rs","line":605,"category":"mutant","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"Node binding is checked only as non-empty, so a nodeSelector on any label and an Exists toleration of every taint pass the lane."},
{"file":"deploy/helm/substrate/values.schema.json","line":283,"category":"acceptance","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"Open podAnnotations admit the AppArmor unconfined annotation for the daemon and init containers, and neither the schema nor the lane refuses it."},
{"file":"xtask/src/chart_yaml.rs","line":311,"category":"contract-drift","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"YAML 1.1 booleans True, yes, on and Y are read as strings while the Kubernetes decoder reads true, so privileged or hostNetwork written that way pass every K4 check."},
{"file":"xtask/src/chart_yaml.rs","line":251,"category":"contract-drift","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"A key with a space before its colon, an anchor or a tag is read with those characters instead of being refused, which hides privileged and hostNetwork from the lane."},
{"file":"xtask/src/chart.rs","line":908,"category":"mutant","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"A Service selecting daemon pods by a controller-added label such as statefulset.kubernetes.io/pod-name is not counted as selecting them."},
{"file":".github/workflows/release.yml","line":321,"category":"acceptance","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"The chart-lane preflight accepts a successful pull_request run, from this repository or a fork, as evidence for a tagged PR-head commit whose tree that run never tested."},
{"file":".github/workflows/release.yml","line":269,"category":"acceptance","severity":"warning","verdict":"CONFIRMED","origin":"pre-existing","message":"The gate preflight accepts a successful pull_request run, from this repository or a fork, as the gate of a tagged PR-head commit."},
{"file":"xtask/src/chart_cluster.rs","line":325,"category":"judgement","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"A cluster lane with an absent egress scenario exits 0, and the release admits that run with nothing in the notes saying a scenario was absent."},
{"file":"xtask/src/chart_cluster.rs","line":968,"category":"judgement","severity":"note","verdict":"INFEASIBLE","origin":"introduced","message":"The undeclared-egress request's result is discarded, so a request refused before dialling would report passed; no path doing that was found."},
{"file":".github/workflows/chart-lane.yml","line":7,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"PR path filters leave out crates/substrate-host, crates/substrate-wire, rust-toolchain.toml and xtask/Cargo.toml, all of which change the image the cluster lane builds."}]
```
