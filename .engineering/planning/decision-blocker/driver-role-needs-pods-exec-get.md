---
format: aep.planning-md/3
id: decision-blocker:driver-role-needs-pods-exec-get
kind: decision-blocker
status: cleared
title: The driver's Role lacks get on pods/exec (ADR 0047 decision 3); operator decides
relations:
- blocks: story:kubernetes-namespace-workspace-and-exec-slice
revision: 2
transitions:
- {from: "open", to: "cleared", at: "2026-10-29T07:23:15Z", actor: "human:timo", revision: 2}
---
# Decision blocker: the driver's Role lacks `get` on `pods/exec` (ADR 0047 decision 3)

Decider: operator. ADR-level; no default on silence.

## Observation

The namespace lane's recorded run (`Namespace-lane: 425ab9c2709b54bc46c234b4646913d8f12f27a2 8
2026-10-29T05:19:33Z`, `kindest/node:v1.37.0`), observation O10: the driver's exec through
`pods/exec` WebSocket `v5.channel.k8s.io` is refused 403 `cannot get resource "pods/exec"`. The
same exec with the lane's admin credentials returns all 200000 bytes and ends `Success`. ADR 0047
decision 3 (`adr/0047-a-kubernetes-namespace-driver-serves-workspaces-on-pvcs-and-execs-as-pods.md`,
line 97) grants `pods/exec` only `create`.

## What it blocks

The keeper path: every workspace file operation (design 34 § 7, N3) runs through `pods/exec`.
Everything else in the slice proceeds.

## Options

| Option | What it does | Cost |
|---|---|---|
| **A** (recommended) | amend ADR 0047 decision 3 by addendum to `get, create` on `pods/exec` | one ADR addendum, the Role line, the rules-review set and its golden; `create` on `pods/exec` is already granted, so `get` adds no reach |
| B | keep `create` only | the keeper path stays unserved; the slice cannot meet its acceptance |
| C | serve the keeper over mutual TLS (ADR 0047 alternative B) | per-keeper certificates, an ingress rule in the default-deny policy, Pod-network reach from the daemon, a new design |

Recommendation: A. It is prepared on branch `side/pods-exec-get`, ready to merge on a yes.

Orchestrator: presented to the operator on 2026-10-29; recorded here at its request.
