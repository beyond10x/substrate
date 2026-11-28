---
format: aep.planning-md/3
id: story:host-driver-supervised-workload
kind: story
status: draft
title: A confined workload on the host driver runs until stopped and restarts by policy
summary: 'Consumer gap 1: the workload family of design 01 section 3.3 on the host driver, confined like an exec, supervised by the daemon, no container daemon.'
owner: substrate
relations:
- decomposes: epic:long-lived-service-on-the-host-driver
- depends_on: story:root-filesystem-from-a-digest-named-archive
revision: 2
---
# Story: a confined workload on the host driver runs until stopped and restarts by policy

## Where the requirement comes from

Consumer gap 1 of the `weft` repository (`cloudspec` `docs/platform-ports.md` § 10, commit
`2ed72c2`): "A long-lived, supervised process from a root filesystem, on the host driver (N3).
First need: every instance that does not end within an exec's bounds: any Service replica, and
ess-system workloads." The stakeholder is the session that maintains `weft`.

## Why

- `docs/design/01-contract.md` § 3.3 designs the workload family (`POST /v1/workloads`, stop,
  start, restart, delete, logs; spec with `restart: never|on-failure|always`; observed status
  `{desired, observed, restarts, last_exit}`); none of it is on the wire (`STATUS.md`).
- `story:docker-image-backed-workload-slice` (proposed) serves it through a Docker daemon only.
- A background exec "has no restart policy" (§ 3.2); a host process (ADR 0036) restarts nothing
  and is unconfined.

## What changes

1. An ADR first (invariant 8), after the operator's answer to decision D1 (workload family on the
   host driver). It fixes the state machine, restart policy and back-off, lease, refusal order,
   events and how a workload is reconciled after a daemon restart.
2. A successor bundle with the workload routes of § 3.3 that the host driver serves: deploy, get,
   list, stop, start, delete, logs. `replace` and `restart` may be deferred by the ADR.
3. The host driver runs a workload's process confined exactly as an exec (the isolation floor of
   `AGENTS.md` § Safety envelope, cgroup bounds, no-egress) with the root filesystem of
   `story:root-filesystem-from-a-digest-named-archive` as `/`, and the daemon supervises it.
4. The workload start carries the exec's `apertures` list (gap 7) and `secret_slots`.

## Acceptance

A weft test (`weft` story `ports-node-workloads-local`) can check each line against a daemon on a
workstation:

- Deploy a workload from an unpacked root filesystem with `restart: never`, a command that runs
  until stopped: observe answers `observed.state: running` within 5 s; stop; observe answers
  `stopped`, and no process of its cgroup remains.
- `restart: on-failure`, a command that exits 1 after 1 s: observe shows `restarts` ≥ 1 and
  `last_exit.code: 1`; a command that exits 0 is not restarted and ends `exited`.
- `restart: always`: a command that exits 0 is restarted; stop ends it and it stays stopped.
- A daemon restart while a workload runs: observe answers the same workload, `running` or
  `observed: null` until probed, never a duplicate process.
- A workload whose lease expires is stopped and its cgroup is empty.
- The workload process cannot open an outbound connection to an undeclared destination, and the
  isolation cases of the delegated lane hold against it.
- Portable lane: without the mechanism the workload fact is absent and a deploy is refused by name
  before dispatch.

## Blocked on / Depends on

- Decision D1 (operator), then the ADR.
- `story:root-filesystem-from-a-digest-named-archive`.
