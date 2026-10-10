---
format: aep.planning-md/3
id: story:aperture-state-outside-workspace-root
kind: story
status: implemented
title: Aperture state is not written into the workspace root
relations:
- decomposes: epic:host-run-gates
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-10T00:48:47Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-10T00:48:47Z", actor: "human:timo", revision: 4}
- {from: "active", to: "implemented", at: "2026-10-10T00:48:59Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"test_result":1}}}
---
# Story: aperture state is not written into the workspace root

## Problem

`HostDriver::open` (`crates/substrate-host/src/lib.rs`) creates `<workspace_root>/.substrate-apertures`
on every open, whether or not an egress aperture is declared. An embedder that uses a directory of
sibling checkouts as its workspace root gets substrate state among those checkouts.

## Design

`HostConfig` gains `aperture_root`, defaulting to the current path so existing deployments keep
their layout. `HostDriver::open` creates it only when `egress_apertures` is non-empty, as it already
does for Git baselines. The daemon places it under its own state directory.

## Acceptance

- Opening a host with no egress aperture creates nothing named `.substrate-apertures` in the
  workspace root.
- A host with an aperture and an explicit `aperture_root` writes its aperture state there and runs
  an aperture exec as before.
- A host with an aperture and the default configuration behaves as on `main`.

## Evidence

Implemented by commit `93c21829a` (merged in https://github.com/beyond10x/substrate/pull/122 as
`a16b12b20`) and released in
[0.7.12](https://github.com/beyond10x/substrate/releases/tag/0.7.12).

- First criterion: `a_host_without_an_aperture_writes_no_aperture_state_in_the_workspace_root`
  (`crates/substrate-host/tests/aperture_root_placement.rs`).
- Second criterion: `an_explicit_aperture_root_holds_the_aperture_state_outside_the_workspace_root`
  (same file); the daemon keeps it beside its state database as `<state>.apertures`
  (`crates/substrate-daemon/src/runtime.rs`).
- Third criterion: `the_default_aperture_root_keeps_the_existing_workspace_layout` (same file).
- Full gate green on the merge commit:
  https://github.com/beyond10x/substrate/actions/runs/38001895521, and on the pull request's head:
  https://github.com/beyond10x/substrate/actions/runs/38001031105.
- Release run https://github.com/beyond10x/substrate/actions/runs/38003936799 succeeded from tag
  `0.7.12` at `7d268e592`, with a 97-case delegated confinement record.
- Not migrated: crash leftovers in `<workspace_root>/.substrate-apertures` from an earlier daemon
  are no longer cleaned by an upgraded daemon, which cleans `<state>.apertures`.
