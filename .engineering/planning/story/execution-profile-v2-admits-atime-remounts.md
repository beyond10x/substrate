---
format: aep.planning-md/3
id: story:execution-profile-v2-admits-atime-remounts
kind: story
status: active
title: Execution profile v2 admits noatime and nodiratime remounts
relations:
- decomposes: epic:kubernetes-deployment-and-driver
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-10-29T02:06:53Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-29T02:06:53Z", actor: "human:timo", revision: 3}
---
# Story: execution profile v2 admits atime remounts

## Outcome

Confined execution runs on a node whose filesystems are mounted `noatime` or `nodiratime`, under a
successor AppArmor profile `substrate-host-exec-v2`, while every byte of the shipped
`substrate-host-exec-v1` stays as released in `0.10.0`.

Decided by D180 (`decision-blocker:execution-profile-noatime-remount`, option A).

## Acceptance

- `deploy/apparmor/` holds `v2`: `v1`'s rules with `noatime` and `nodiratime` added to the
  `remount options in (…) /**` set, and nothing else changed. `v1` is byte-identical to `0.14.0`.
- The installer's exact-source check, the bootstrap's name check, the container profile tools and
  the chart default accept `v2`; the profile name stays a deployment parameter (design 33).
- A profile-level test diffs the two rule sets: every remount flag combination `v1` refuses, `v2`
  refuses too, except those adding `noatime` or `nodiratime`. A mutant `v2` that admits any other
  flag (for example `rw` without `ro`, or dropping `nosuid`) fails it.
- `CHANGELOG.md` names `v2` and the upgrade step: nodes load `v2`.
- The chart lane (PR 190's `cargo xtask chart-lane cluster --node-profiles`) runs green under `v2`
  on a `noatime` node and its `Chart-lane:` record names `v2`.
- The deployment guide keeps "Not verified: whether production nodes mount `noatime`".

## Out of scope

Removing `v1`; any other profile rule.
