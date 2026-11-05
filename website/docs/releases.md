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

## 0.20.0 (2026-11-05)

The wire, the contract bundle (`substrate-wire/0.26.0`) and every published route are unchanged.
`b10x-substrate-host` gains two default features, so a consumer that needs only confinement can
leave out Git and Kubernetes. The two remaining egress reaps retry through an interrupted wait, and
the ESS pin moves to 0.86.0.

### Added in 0.20.0

- `b10x-substrate-host` declares the features `git` (gix, git2 and curl) and `kubernetes`
  (`b10x-substrate-kubernetes-client`), both on by default, so every existing consumer, the daemon
  included, builds and behaves as before.
  - A consumer that needs only `substrate_host::confinement` sets `default-features = false`. Its
    normal dependency tree then holds 116 packages instead of 315, with none of gix, git2, curl,
    openssl, rustls, aws-lc or the Kubernetes client.
  - Without `git`, the probe publishes no `workspace.git`, and a Git source or Git root is refused
    as unserved.
  - Without `kubernetes`, opening the `pod` adapter is refused `confinement.adapter-unserved`,
    naming the feature.
  - Every public item is the same under every feature set.
  - `cargo xtask check-packages` admits exactly these two host features, with both as the
    default.
  - The gate lints the host crate with no default features and runs its `host_features`,
    `ess_confinement_local` and `consumer_surface` cases in that build.
  - The CLI's dependency on the host crate takes no default features; the CLI uses neither.

### Fixed in 0.20.0

- Dropping an installed egress aperture reaps its forwarder even when a signal handler installed
  without `SA_RESTART` interrupts the wait.
- The startup probe that proves the egress mechanism reads its child's real exit status, so an
  interrupted reap, or a child ended by a signal, is never read as "reached".
- All three reaps share one helper that waits again on `EINTR` and ends on any other error.

### Changed in 0.20.0

- The ESS pin is 0.86.0 (`spec/*/ess-inputs.yaml`, and `ESS_VERSION` and `ESS_SHA256` in
  `gate.yml`). The committed suites are regenerated: their refusals are unchanged, and 0.86.0 adds
  `expect_no_error` steps.
