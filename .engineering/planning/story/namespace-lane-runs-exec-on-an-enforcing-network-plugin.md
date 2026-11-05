---
format: aep.planning-md/3
id: story:namespace-lane-runs-exec-on-an-enforcing-network-plugin
kind: story
status: proposed
title: The namespace lane runs its exec cases on a network plugin that enforces NetworkPolicy
owner: substrate
tags:
- kubernetes
- lane
relations:
- depends_on: story:namespace-driver-admits-an-exec-on-observed-confinement
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-10-31T11:27:59Z", actor: "human:timo", revision: 2}
---
# Story: the namespace lane runs its exec cases on a network plugin that enforces NetworkPolicy

## Outcome

`cargo xtask namespace-lane observe` creates its kind cluster with a pinned network plugin that
enforces NetworkPolicy, so the driver's probe observes `confinement.no-egress` there, and the lane's
exec cases run instead of being reported absent.

## Design

[Design 34](../../../docs/design/34-kubernetes-namespace-driver.md) § 10.2 and § 10.3 O3: kindnet
does not enforce NetworkPolicy, so O3 did not hold and no exec could be admitted on the lane.
Filed under option A of `decision-blocker:namespace-exec-needs-design-30`, § *What follows A*,
item 3 ("the lane installing a pinned enforcing CNI").

## Acceptance

- The lane's kind cluster disables the default plugin and installs one plugin that enforces
  NetworkPolicy, pinned by version and image digest; the lane record names it.
- O3 holds on that cluster: the probe connections fail under the default-deny policy and the
  control Pod makes all three.
- The machine read in step 3 states `confinement.no-egress` and the seven families of design 34
  § 6.2; an exec start is admitted.
- The four exec cases now in `ABSENT` (`S6 quota exec.capacity`, `S6 restart: running exec reads
  unknown and its Pod is gone`, `S6 force-deleted Pod is not reported ended early`, `S6 Role
  without pods watch`) are executed and pass; the remaining absent cases keep their reasons.
- A recorded lane run for the tagged commit of the release that ships it.

## Boundaries

- Which plugin a deployment runs stays the operator's; the lane chooses one only so the observation
  can hold.
- Depends on `story:namespace-driver-admits-an-exec-on-observed-confinement`.

## Spike: Calico on the lane's kind node (2026-11-04)

Approved as the wave's U4 (`approval-record:wave-2026-11-04-substrate`); nothing is accepted from it.
A throwaway local probe, not a lane run: kind v0.33.0 with the lane's pinned
`kindest/node:v1.37.0@sha256:a1ed56cfb0e7b93589bdf97c8cd566405a265939e3620fc4f5de89adff580ae5`,
the default plugin disabled, Calico `v3.33.0` from its release manifest (sha256 `2de8f475…a234da5`;
images `quay.io/calico/node@sha256:565d8db3…` and `quay.io/calico/calico@sha256:4ebc2073…`) in
its default iptables mode, and one `NetworkPolicy` selecting the probe Pods with both policy types
and no rules. Each busybox Pod connected with `nc -w 3` to the Service host `10.96.0.1:443`, the
nameserver `10.96.0.10:53` and `1.1.1.1:443`, in that order, timing each attempt.

| Pod | first attempt | Service host | nameserver | outside |
|---|---|---|---|---|
| control (not selected) | at start | connected, 10 ms | connected, 0 ms | connected, 50 ms |
| probe | at start | failed after 3010 ms | failed after 3000 ms | failed after 3000 ms |
| probe | after 10 s | failed after 3010 ms | failed after 3000 ms | failed after 3000 ms |
| probe (second) | at start | failed after 3000 ms | failed after 3000 ms | failed after 3000 ms |

Observed: Calico enforced the policy from each Pod's first instant in all three probe Pods, and
the control Pod reached all three targets. Every denied attempt ran out the 3 s bound, so the
denial was silent. A local `EPERM` or `EACCES` answers at once, and none did here. busybox `nc`
prints no errno on a timeout; the elapsed time is the evidence.

Consequence (inferred from design 34 § 6.2 and the amendment at line 882, not run through the
driver): on this plugin the driver's probe would see three timeouts, which prove nothing, so
`confinement.no-egress` stays absent and no exec is admitted. Installing Calico alone does not
meet this story's acceptance. A plugin that denies inside the Pod, or reading silence safely
(`story:namespace-probe-repeats-and-sticks-on-egress`), is still needed. That story's inferred
early-egress window did not appear on Calico in three Pods. The window was seen on kind's default
plugin.

Log: kept outside the repository by the wave lead; the cluster was deleted at 08:57:55Z.
