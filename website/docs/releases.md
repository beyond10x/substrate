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

## 0.19.0 (2026-11-01)

The wire, the contract bundle (`substrate-wire/0.26.0`) and every published route are unchanged.
The Kubernetes namespace driver names the verb behind a bare exec refusal and refuses an exec
whose keeper is on a node its probe did not observe; an egress aperture's listener is held only by
its forwarder, and the `pod` adapter keeps a `NetworkPolicy` written after its create.

### Changed in 0.19.0

- The Kubernetes namespace driver names the verb behind a bare 403 exec refusal. When the API
  refuses the `pods/exec` handshake with no reason after `is forbidden:`, the driver asks a
  `SelfSubjectAccessReview` for `pods/exec` `create` and `get` on the keeper, at most two extra
  API calls within 5 s. It appends the verb the review denies, or says that the review did not
  decide; a refused or failed review keeps the API's answer and names the verb unknown. The
  handshake still ends at once, and no Role gains a rule (design 34 § 6.1 step 7, § 7.1).
- The ESS pin moves from 0.77.0 to 0.80.0.

### Fixed in 0.19.0

- The Kubernetes namespace driver refuses an exec start with `exec.sandbox-unavailable` (501),
  before it creates any Pod, when the workspace keeper is on a node other than the one its latest
  capability probe observed. The node is read after the start's policy re-read, so a probe
  published elsewhere in the meantime also refuses it. A probe whose keeper node could not be read
  proves no run fact. The snapshot's facts and the wire are unchanged.
- An egress aperture's listener is no longer held by the installing daemon: the forwarder makes
  its own and keeps it, so a process forked during an install cannot keep a dead forwarder's
  aperture accepting connections from inside.
- The `pod` adapter's sweep no longer removes a `NetworkPolicy` as "Pod never created" when the
  policy's `managedFields` show a write after its create (a second entry, or the one entry later
  than `creationTimestamp`); it keeps the policy and logs it. A policy unchanged since its create,
  or listed without `managedFields`, is swept as before.
