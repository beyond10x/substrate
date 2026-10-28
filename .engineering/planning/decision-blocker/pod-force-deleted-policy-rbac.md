---
format: aep.planning-md/3
id: decision-blocker:pod-force-deleted-policy-rbac
kind: decision-blocker
status: cleared
title: How the pod adapter tells a force-deleted Pod's container is gone
relations:
- blocks: story:pod-force-deleted-keeps-its-policy
revision: 3
transitions:
- {from: "open", to: "cleared", at: "2026-10-28T21:18:38Z", actor: "human:timo", revision: 3}
---
# Decision: how the pod adapter tells a force-deleted Pod's container is gone

Decider: orchestrator (D173, under the operator's grant)

## Why it stops here

`story:pod-force-deleted-keeps-its-policy` (finding J2 of
`review-result:pod-unknown-cleanup-security-pass-1`): on a 404 for a Pod read, the monitor, the open
sweep and the deferred sweep delete the run's NetworkPolicy. A Pod removed by a forced delete while
its node was unreachable reads 404 while its container may still run.

No fail-closed rule works with the Role design 28 grants (`create,get,list,delete` on
`pods,networkpolicies`, `get` on `pods/log`, one namespace; `.github/workflows/gate.yml:236-237`):

- node state needs `nodes`, which is cluster-scoped and not granted;
- the rendered Pod has no finalizer, so a forced delete never shows `deletionGracePeriodSeconds: 0`
  to a read, and the kubelet's own final delete is grace 0 as well;
- a terminal container status is seen by a 200 read only for runs that end on their own; after the
  adapter's own kill the kubelet writes the status and deletes the Pod in one sync, so keeping the
  policy would leave objects that `a_killed_run_ends_on_sigkill_and_leaves_nothing` and the gate's
  "Require that no run object remains" step require gone.

Defect sites: `pod.rs:572`, `pod.rs:636-639`, monitor `pod.rs:1762-1810`, open sweep
`pod.rs:822-831`, deferred sweep `pod.rs:984-1008`.

## Options

| option | what it does | cost |
|---|---|---|
| A (recommended) | add `watch` on `pods` to the namespaced Role; on a 404, one bounded watch from the last seen resourceVersion reads the DELETED object; the policy goes only on a terminated container or a `Succeeded`/`Failed` phase, and is kept on 410, timeout or a running container | changes the Role design 28 documents; needs one kind-lane run first to show the kubelet writes the terminal status before its final delete |
| B | add `get` on `nodes` (ClusterRole and binding); release the policy when the Pod's node reads `Ready=True` with a heartbeat later than the 404 | a cluster-scoped grant design 28 avoids; policy removal after a kill waits up to about 5 minutes |
| C | record the accepted risk in design 28 with its reason | no code; a force-deleted Pod on a partitioned node keeps running without its egress policy |

The adapter never force-deletes and run names are unique, so no path inside the adapter reaches
the defect.

## Decided: A (D173, 2026-10-28)

The coordinating session's message, verbatim:

> D173: I decide decision-blocker:pod-force-deleted-policy-rbac under the grant: A. Change its
> Decider line to the orchestrator, quote this message and close it.
> - Why mine: a security trade-off inside the approved design 28, not a never-list item. Timo's
>   rule (~/.claude/CLAUDE.md): "take the decisions yourself (defaults, sequencing, security
>   trade-offs inside an approved design)".
> - `watch` on pods in the same namespaced Role adds no read the Role lacks today: it already has
>   get and list on pods.
> - Steps:
>   1. Run the kind lane once to confirm the kubelet writes terminal status before the final
>      delete. That is local and needs no device.
>   2. Add `watch` to the Role.
>   3. A bounded watch reads the DELETED object; keep the policy on 410, timeout or a running pod.
>   4. Record the new verb in design 28's Role table and in the chart's CHANGELOG as a required
>      RBAC change for deployments that pre-create the Role.
> - B rejected: a ClusterRole widens scope beyond the namespace, plus a delay of about 5 min.
> - C rejected: it leaves the gap open.
> - If the kind run shows the kubelet does not write terminal status first, stop and send me the
>   run output.
> Unit 4 goes back into wave/adapter-io.
