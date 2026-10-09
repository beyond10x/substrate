---
format: aep.planning-md/3
id: story:aperture-state-outside-workspace-root
kind: story
status: draft
title: Aperture state is not written into the workspace root
relations:
- decomposes: epic:host-run-gates
revision: 1
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
