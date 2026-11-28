---
format: aep.planning-md/3
id: epic:long-lived-service-on-the-host-driver
kind: epic
status: draft
title: A long-lived service runs from a root filesystem on the host driver
summary: 'Consumer gaps 1-7 of weft: workload, root filesystem, inbound and local endpoints, volume, per-run secret, egress, all without a container daemon.'
owner: substrate
revision: 3
---
# Epic: a long-lived service runs from a root filesystem on the host driver

## Problem

A consumer runtime (the `weft` repository, the runtime of `cloudspec`) needs to run a long-lived
service from an image on a workstation, with no container daemon. Its gap list
(`cloudspec` `docs/platform-ports.md` § 10, commit `2ed72c2`) names seven Substrate gaps. Today:

- Background execs "have no restart policy — that is what workloads are for"
  (`docs/design/01-contract.md` § 3.2), and workloads, images, volumes and endpoints are absent from
  the wire (`STATUS.md`, "Workloads, images, volumes, endpoints and the Docker and Firecracker
  drivers are absent from the wire").
- The only planned workload is Docker-backed (`story:docker-image-backed-workload-slice`, proposed).
- Host processes (ADR 0036) are supervised but unconfined (`profile: host`, `confinement: none`).
- A confined exec's network namespace has loopback only; nothing carries a connection into it
  (`story:inbound-endpoint-for-a-confined-exec`).
- Secret slots resolve from daemon-configuration secrets only (`docs/design/01-contract.md` § 3.3,
  `story:sealed-secret-slots`).

## Outcome

On the host driver, without a container daemon, a caller can: unpack a root filesystem from a
gzip tar named by its sha256 digest; run a confined, daemon-supervised workload from it under a
restart policy; expose one of its ports at an observed loopback address; give it a local endpoint
whose connections the node agent can attribute to that workload; mount a read-write volume that
outlives the run; deliver a secret value for that run only; and name operator-declared egress
apertures.

## Gaps and stories

| gap | need | story |
|---|---|---|
| 1 | long-lived supervised process from a root filesystem | `story:host-driver-supervised-workload` |
| 2 | inbound endpoint into a confined exec or workload | `story:inbound-endpoint-for-a-confined-exec` |
| 3 | per-run local endpoint attributed to its run | `story:local-endpoint-attributed-to-its-run` |
| 4 | secret value a caller gives one run at its start | `story:a-start-delivers-a-per-run-secret-value` |
| 5 | root filesystem from a digest-named archive | `story:root-filesystem-from-a-digest-named-archive` |
| 6 | read-write volume kept across runs | `story:read-write-volume-kept-across-runs` |
| 7 | egress destinations a run names within operator bounds | served since bundle `0.21.0` (`review-result:weft-gap-7-served-by-named-apertures`) |

## Decisions

Gaps 1, 2, 4 and 5 change Substrate's architecture (a new resource family on the host driver, a
new inbound trust boundary, a secret value in a request, a new untrusted-input parser). They are
put to the operator as lettered options; no code starts before each answer is recorded as an
approval record and its ADR is accepted (invariant 8). Gap 3 and gap 6 are design changes within
ADR 0013 and `docs/design/01-contract.md` § 3.5.

## Order

The consumer needs 1, 2, 3 and 5 first, then 4 and 7. 5 comes before 1 (a workload runs from a
root filesystem), 2 and 3 can run beside 1 for execs and are wired to workloads when 1 lands, 6
after 1.

## Out of scope

The Kubernetes namespace driver (a later consumer gap), OCI images (second input, after the gzip
tar), image builds and pushes, registries, fleet scheduling.
