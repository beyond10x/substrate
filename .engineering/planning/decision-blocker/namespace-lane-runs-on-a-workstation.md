---
format: aep.planning-md/3
id: decision-blocker:namespace-lane-runs-on-a-workstation
kind: decision-blocker
status: cleared
title: The namespace kind lane runs as a workstation record, not a CI job (D182)
relations:
- blocks: story:kubernetes-namespace-workspace-and-exec-slice
revision: 2
transitions:
- {from: "open", to: "cleared", at: "2026-10-29T04:41:45Z", actor: "human:timo", revision: 2}
---
# Where the namespace kind lane runs

## Question

Design 34 § 10.2 (N11 A) puts the namespace kind lane in CI as a `cluster` job in
`.github/workflows/namespace-lane.yml`. The operator's rule for this work caps CI growth at one
minute per workflow, and a kind cluster job takes several minutes.

## Decision (D182, 2026-10-29)

Proposed by the substrate wave session, accepted by the coordinating session under the operator's
grant, verbatim:

> Accepted (D182): U5 ships the `static` CI job, and the cluster part runs as `cargo xtask
> namespace-lane observe`, following the chart-lane precedent and the 1-minute CI cap you cite. One
> condition, so the lane doesn't go stale: the release for this wave refuses unless a
> `Namespace-lane:` record for the exact release head is present, as the chart lane's record is.
> Run the observe verb on the integration head before the PR, and put the record line in the PR
> body and the tag message. Record this in your store, with the § 10.2 amendment in U13.

## What follows

- U5: `namespace-lane.yml` holds the `static` job only; `cargo xtask namespace-lane observe` runs on
  a workstation and prints `Namespace-lane: <sha> <count> <RFC 3339 UTC>` only when every
  observation ran at a clean, unmoved HEAD.
- U12 (release preflight): `release.yml` refuses a tag without a `Namespace-lane:` record naming
  the tagged commit, with the same one-line, three-field and calendar rules as `Chart-lane:`.
- Before this wave's pull request: run the observe verb on the integration head; the record line
  goes in the PR body and, at release, in the tag message.
- U13: design 34 § 10.2 is amended to say the cluster part is a workstation record.
