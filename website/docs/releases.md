---
title: Release notes
description: What changed in the newest Substrate release, generated from CHANGELOG.md.
---

# What changed in the newest release

This page ends with the newest released section of `CHANGELOG.md`, and `cargo xtask check-docs`
fails when the two differ. Earlier releases are in the
[full changelog](https://github.com/beyond10x/substrate/blob/main/CHANGELOG.md). The
[confinement adapters](./concepts/adapters.md) page explains each adapter the notes name.

What 0.16.1 first publishes is listed under 0.16.0 in the full changelog and summarized on the
[status page](./status.md#new-in-0161), including the namespace driver's limits.

<!-- Generated from CHANGELOG.md by `cargo xtask docs-releases`. Edit CHANGELOG.md, not the text below. -->

## 0.19.1 (2026-11-02)

Fixes only; the wire, the contract bundle (`substrate-wire/0.26.0`) and every published route are
unchanged. The Kubernetes namespace driver no longer reads an undecided access review as a denial or
publishes an older probe over a newer one, the `pod` adapter sweeps a written `NetworkPolicy` once a
replay shows its run stopped, and an egress aperture install sees its forwarder die at once.

### Changed in 0.19.1

- The ESS pin moves from 0.80.0 to 0.81.0.

### Fixed in 0.19.1

- The Kubernetes namespace driver no longer reads a `SelfSubjectAccessReview` that answers
  `allowed: false` with a `status.evaluationError`, and no `denied: true`, as a denial. A bare exec
  refusal names any verb an earlier review denied, then says that this review does not decide the
  refused verb and quotes the API's evaluation error, bearer token cleared and bounded to 300
  characters (design 34 § 7.1).
- Two snapshot calls that overlap in the Kubernetes namespace driver no longer publish an older
  probe's snapshot, generation and node over a newer one; the snapshot is computed and published
  under one lock.
- The `pod` adapter's sweep again removes a `NetworkPolicy` written after its create once a watch
  replay from the policy's own `resourceVersion` shows its run's container terminated; a replay
  that ends in a `BOOKMARK` alone still keeps it (design 28).
- A `NetworkPolicy` the `pod` adapter keeps because it was written after its create logs one
  `warn` that names the write, and the `info` line every open sweep logged for every written policy
  is gone.
- An egress aperture install whose forwarder dies before its report is refused at once, even when
  a child that another thread forked during the install still holds the report socket: the
  installer waits on the forwarder's pidfd beside the socket. A pidfd that cannot be opened refuses
  the install with `exec.aperture-install-failed`.
