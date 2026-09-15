---
format: aep.planning-md/1
id: release-plan:substrate-0-7-2
kind: release-plan
status: implemented
title: Release Substrate 0.7.2
revision: 4
---
# Release plan: Substrate 0.7.2

Publish the bounded runtime-image correction from a fully gated main commit using an annotated bare-version tag. The released contract bundle remains byte-identical.

## Record

No `0.7.2` tag was cut and no GitHub release exists (`git ls-remote --tags origin` lists 0.7.0, 0.7.1, 0.7.3 …; `gh api releases/tags/0.7.2` → 404). The runtime-library correction this plan covers (`7594b1a`, `CHANGELOG.md` § 0.7.2, 2026-09-05) shipped inside `0.7.3` the same day. The status `implemented` records that the change reached a release, not that this plan's own tag was published (reviewer D2, 2026-09-15).
