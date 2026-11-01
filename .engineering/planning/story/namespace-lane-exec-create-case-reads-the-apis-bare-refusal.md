---
format: aep.planning-md/3
id: story:namespace-lane-exec-create-case-reads-the-apis-bare-refusal
kind: story
status: active
title: The pods/exec create case reads the API's bare refusal
relations:
- decomposes: epic:confinement-contract-family
- informed_by: approval-record:d349-substrate-wave-2026-11-01
scope:
- confidence: cited
  path: xtask/src/namespace_lane_run.rs
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-01T10:16:17Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-11-01T10:16:17Z", actor: "human:timo", revision: 4}
---
# Story: the pods/exec create case reads the API's bare refusal

## Observation

The 0.18.1 namespace lane at `bb2b06fe` (2026-11-01, 10:00-10:13Z) gave no record on
"S6 Role without pods/exec create". The daemon log the lane kept shows the executor handshake
refused at once with the API's whole `Status.message`, which carries no reason:

```
2026-11-01T10:08:42.729145Z  WARN substrate_kubernetes::namespace::probe: the namespace driver's probe failed; its facts are absent code="kubernetes.executor-handshake-failed" the cluster API: exec answered 403: pods "sbx-pr-gbyb7pajquihb757-keeper" is forbidden: 
```

The rule from `story:namespace-lane-verb-case-accepts-only-its-own-refusal` waits for
`cannot create resource "pods/exec"`, which this API does not send. The "pods/exec get" case
carries `cannot get resource "pods/exec"` and passes.

## Acceptance

- A red case first, on the observed line: the "pods/exec create" case is refused by a `WARN` line
  with `code="kubernetes.executor-handshake-failed"` whose `exec answered 403` names the probe's
  keeper Pod (`sbx-pr-…-keeper`) and gives nothing after "is forbidden:", once the probe has
  finished.
- Such a line refuses no other verb case; the "pods/exec get" case still needs
  `cannot get resource "pods/exec"`.
- The two cases stay distinct: the get case's line does not refuse the create case and the bare
  line does not refuse the get case; a bare line naming another Pod, another code, another status,
  or carrying any `cannot … resource` text refuses nothing.
