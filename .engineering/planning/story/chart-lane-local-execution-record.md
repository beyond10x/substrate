---
format: aep.planning-md/3
id: story:chart-lane-local-execution-record
kind: story
status: implemented
title: A recorded local lane proves the execution-enabled serving pod (ADR 0046 K9 C)
summary: A workstation kind lane with node profiles starts the execution-enabled pod and prints a Chart-lane record that release.yml requires.
relations:
- informed_by: story:node-bound-kubernetes-serving-profile
- decomposes: epic:kubernetes-deployment-and-driver
- depends_on: story:host-profile-name-is-a-deployment-parameter
- depends_on: story:execution-profile-v2-admits-atime-remounts
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-29T12:18:59Z", actor: "human:timo", revision: 2, decided_on: {"recorded":{"test_result":1}}}
- {from: "proposed", to: "active", at: "2026-10-29T12:18:59Z", actor: "human:timo", revision: 3, decided_on: {"recorded":{"test_result":1}}}
- {from: "active", to: "implemented", at: "2026-10-29T12:18:59Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":2}}}
---
# Story: a recorded local lane proves the execution-enabled serving pod

## Outcome

ADR 0046 decision K9 names three lanes for the node-bound Kubernetes serving profile. K9 B, the
hosted kind lane in `chart-lane.yml`, cannot start a pod with execution enabled: a hosted runner has
no Localhost seccomp or AppArmor profile installed on its node, so the bootstrap capabilities, the
Localhost profiles and the 0400 key read after the uid drop are only checked by a server dry-run.
`story:node-bound-kubernetes-serving-profile` reports those scenarios as absent, by name.

This story adds K9 C: a local lane on a workstation kind node with the node profiles installed. It
starts the execution-enabled StatefulSet, runs one confined exec through it, reads the key after the
uid drop, and prints one line in the same shape as `Confinement-lane:` and `Quota-lane:`:

```
Chart-lane: <source-sha> <scenario-count> <RFC 3339 UTC>
```

`release.yml` refuses a tag without that line, with the same one-line, three-field, calendar and
commit checks as the other two records, and writes it into the release notes beside the K9 B run.

## Acceptance

- The lane refuses with a named prerequisite when the node profiles are absent, and names every
  scenario that therefore did not run; it never reports green having run nothing.
- The scenario count in the record equals the count the tagged source declares.
- `xtask/tests/release_workflow.rs` executes the release step's refusals offline.

## Scope

`xtask/src/chart_cluster.rs`, `.github/workflows/release.yml`, `xtask/tests/release_workflow.rs`,
AGENTS.md § Releases.
