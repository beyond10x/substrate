---
format: aep.planning-md/3
id: story:host-profile-name-is-a-deployment-parameter
kind: story
status: active
title: The host execution profile name is a deployment parameter
relations:
- decomposes: epic:kubernetes-deployment-and-driver
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-10-28T22:17:15Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-28T22:17:16Z", actor: "human:timo", revision: 3}
---
# The host execution profile name is a deployment parameter

## Why

`decision-blocker:chart-lane-host-profile`, decided A (D167): the local chart lane shares the
host kernel, which may already hold `substrate-host-exec-v1` for another workload, so the lane
needs its own profile name. The name is fixed today in `deploy/apparmor/substrate-host-execution`,
compiled into the node-profile installer for its exact-source check, fixed in
`crates/substrate-daemon/src/container_profiles.rs` and checked by the bootstrap in
`crates/substrate-daemon/src/container_bootstrap.rs` and the chart's StatefulSet.

## Design first

A design document states how the policy is rendered from one template with the name, how the
installer's exact-source check compares against the rendered bytes, how the bootstrap learns the
configured name, and the chart value. It stops and is filed for the operator if it finds the
change needs an ADR or alters what a deployed profile enforces.

## Acceptance

- With the parameter unset, the rendered policy, the installer, the bootstrap check and
  `helm template` output are byte-identical to today's.
- With a name set, the installer loads exactly that profile from the rendered source and refuses
  any other, and the bootstrap refuses a label other than `<name> (enforce)`.
- `cargo xtask chart-lane cluster --node-profiles` uses a lane-unique name, never touches a
  profile it did not load, and prints a `Chart-lane:` record on this workstation.
