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

## 0.22.0 (2026-11-20)

The contract bundle moves to `substrate-wire/0.27.0`, which adds no route and raises three
bounds: one run selects up to eight egress apertures, a deployment declares up to twelve, and a
start declares up to eight read-only roots. A client pinned to `0.26.0` must move with it.

### Changed in 0.22.0

- One run selects up to eight egress apertures (`MAX_EGRESS_APERTURES`, was four), a deployment
  declares up to twelve (`MAX_DEPLOYMENT_EGRESS_APERTURES`, was six and now a wire constant), and a
  start declares up to eight read-only roots (`MAX_READ_ONLY_ROOTS`, was four), by the 2026-11-20
  amendments of ADR 0013 and ADR 0010 (#236). A GitHub box already selected four apertures and
  could add no package-registry egress. The daemon, the SDK and the conformance runner advertise
  the successor bundle `substrate-wire/0.27.0` (inner `bundle.json` sha256
  `b55318e0570a116776839f2bf105f017737b642cbd71027d1fe5d20e785ff3b7`), which adds no route and
  publishes the three bounds; its `exec.egress-apertures` fact list is bounded by the deployment
  bound, where `0.26.0` and earlier bounded it by the run's four and so refused the snapshot of a
  daemon declaring five or six. A client pinned to `0.26.0` must move with it. Bundles `0.21.0` to
  `0.26.0` keep their bytes; their sources state the old bound as a literal.
- The release reads the four lane records (confinement, quota, chart node profiles, namespace) from
  `release-lanes.yml`'s hosted run for the tagged commit; a recovery input and then a tag trailer
  still take precedence per lane.

### Fixed in 0.22.0

- The systemd local-run case of the delegated lane holds its leader under dash: a fork refused at
  `pids.max` no longer ends the leader on a Debian-family host (#230). A test defect; the adapter
  and the host were right.
- The gate's tests phase fetches every target's dependencies before the workspace tests, so a cold
  runner cache no longer fails the offline `cargo tree --target all` case.
