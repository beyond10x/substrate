---
format: aep.planning-md/3
id: story:driver-names-a-bare-exec-refusals-verb
kind: story
status: proposed
title: The driver names a bare exec refusal's verb
relations:
- decomposes: epic:confinement-contract-family
- informed_by: approval-record:d349-substrate-wave-2026-11-01
scope:
- confidence: inferred
  path: crates/substrate-kubernetes/src/namespace/workspace.rs
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-01T10:16:18Z", actor: "human:timo", revision: 3}
---
# Story: the driver names a bare exec refusal's verb

## Observation

On kind v1.37 the API refuses an exec upgrade the Role lacks `pods/exec` `create` for with a
`Status` whose message carries no reason (the 0.18.1 namespace lane at `bb2b06fe`, daemon log kept
by the lane):

```
2026-11-01T10:08:42.729145Z  WARN substrate_kubernetes::namespace::probe: the namespace driver's probe failed; its facts are absent code="kubernetes.executor-handshake-failed" the cluster API: exec answered 403: pods "sbx-pr-gbyb7pajquihb757-keeper" is forbidden: 
2026-11-01T10:09:43.181817Z  WARN substrate_kubernetes::namespace::probe: the namespace driver's probe failed; its facts are absent code="kubernetes.executor-handshake-failed" the cluster API: exec answered 403: pods "sbx-pr-fma7x5qkyobdhq4t-keeper" is forbidden: 
```

An operator reading that warning cannot tell which permission is missing. D349 accepts the bare
line in the lane for that one case; this story makes the driver name the verb.

## Acceptance

- When an exec is refused 403 with no reason after "is forbidden:", the driver asks a
  `SelfSubjectAccessReview` for `pods/exec` `create` and `get` in its namespace and names the verb
  the review denies in the refusal, or says the review did not decide.
- The review's own refusal or failure keeps the original refusal and names that the verb is
  unknown; the handshake still ends at once.
- Design 30 states the extra API call; the chart's Role and the namespace lane's Role need no new
  rule if the cluster's default `system:basic-user` binding admits the review, and the story says
  which it found.
