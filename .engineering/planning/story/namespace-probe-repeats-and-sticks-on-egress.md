---
format: aep.planning-md/3
id: story:namespace-probe-repeats-and-sticks-on-egress
kind: story
status: proposed
title: The namespace probe repeats its egress attempts and withholds no-egress once any got through
owner: substrate
tags:
- confinement
- kubernetes
relations:
- informed_by: approval-record:d333-timeout-never-proves-no-egress
- depends_on: story:namespace-probe-never-proves-no-egress-from-a-timeout
- informed_by: approval-record:d334-only-a-local-denial-proves-blocked
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-31T18:33:19Z", actor: "human:timo", revision: 2}
---
# Story: the namespace probe repeats its egress attempts and withholds no-egress once any got through

## Outcome

On a network plugin that drops denied packets silently, the namespace driver can still state
`confinement.no-egress`, by repeating its probe in several fresh Pods over time, each attempting at
its first instant, and publishing the fact only when every attempt of every run timed out. Any
attempt reached or refused withholds the fact for the binding generation.

## Design

Option B of D333 (`approval-record:d333-timeout-never-proves-no-egress`). The evidence is the two
D332 traces at `3b3998c8` (`approval-record:d332-no-egress-fix-wave`):

- Trace 1: Kubernetes Service reached in 0 ms, nameserver reached in 21 ms, egress probe address
  refused (errno 111).
- Trace 2: all three attempts timed out (3001 ms, 3058 ms with errno 11, 3001 ms), while the lane's
  own Pod under the same default-deny policy reached all three targets.

Inferred, not verified: kind's network plugin enforces NetworkPolicy only after a Pod's first
moments, so an exec Pod may egress early. The design 34 amendment this needs, including how the
start window is bounded, is decided when this story is picked, not now.

## Acceptance

To be stated with the design 34 amendment.

## No route since D334

Since D334 (`approval-record:d334-only-a-local-denial-proves-blocked`), only `EPERM` or `EACCES`
counts as blocked: `ENETUNREACH` and `EHOSTUNREACH` can come from a remote router's ICMP host or
net unreachable or from an address of the wrong family. This story also decides whether reading
an attempt's errno together with its elapsed time can tell a local no-route (answered at once,
inside the Pod) from a remote one, and if so how `no_egress()` reads it.

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
