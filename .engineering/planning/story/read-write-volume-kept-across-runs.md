---
format: aep.planning-md/3
id: story:read-write-volume-kept-across-runs
kind: story
status: draft
title: A read-write volume is kept across runs
summary: 'Consumer gap 6: the volume family of design 01 section 3.5 on the host driver.'
owner: substrate
relations:
- decomposes: epic:long-lived-service-on-the-host-driver
- depends_on: story:host-driver-supervised-workload
revision: 2
---
# Story: a read-write volume is kept across runs

## Where the requirement comes from

Consumer gap 6 of the `weft` repository (`cloudspec` `docs/platform-ports.md` § 10, commit
`2ed72c2`): "A read-write volume mounted into an exec or workload (N5)." Needed by `weft` and by
the `slot` repository's workloads. The stakeholder is the session that maintains `weft`.

## Why

- `docs/design/01-contract.md` § 3.5 designs volumes (create, list, delete; deletion refused while
  mounted; mounts `{target, path, ro}`); they are absent from the wire.
- A writable root (ADR 0038) binds a declared host directory or a Git workspace, not a
  Substrate-owned volume with its own lifecycle.

## What changes

Within § 3.5, decided here; a design document fixes it before code (invariant 8):

1. A successor bundle with `POST|GET|DELETE /v1/volumes[/{id}]`, a capability fact and named
   refusals; a volume is an owner-private directory under the daemon's state, optionally under a
   project quota where the host serves one.
2. Exec, session and workload starts may mount a volume read-write or read-only at a path; delete
   is refused `conflict` while a run mounts it.

## Acceptance

A weft test (`weft` story `ports-node-volumes-local`) can check:

- Create a volume: it answers `available`. Run A mounts it read-write at `/data` and writes a
  file; after A ends, run B mounts it and reads the file.
- Delete while B runs is refused `conflict`; after B ends, delete succeeds and the directory is
  gone from the host.
- A run mounting it read-only cannot write to it.
- Portable lane: without the mechanism the fact is absent and a create is refused by name.

## Blocked on / Depends on

- A design document before code. Workload mounts with `story:host-driver-supervised-workload`.
