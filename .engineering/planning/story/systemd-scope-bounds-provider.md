---
format: aep.planning-md/3
id: story:systemd-scope-bounds-provider
kind: story
status: draft
title: 'The systemd adapter: a delegated user scope as the bounds provider'
owner: substrate
tags:
- adr-0109
relations:
- decomposes: epic:confinement-contract-family
- depends_on: story:confinement-contract-on-linux-and-local
scope:
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: crates/substrate-host/src/confinement/mod.rs
- confidence: inferred
  path: crates/substrate-host/src/confinement/systemd.rs
- confidence: inferred
  path: crates/substrate-host/tests/ess_confinement_systemd.rs
- confidence: inferred
  path: scripts/delegated-lane.sh
revision: 2
---
# The `systemd` adapter: a delegated user scope as the bounds provider

ADR 0109 decision 3: `systemd` is a scope as the bounds provider. Today loom, takt, conductor and
metaharness each call `systemd-run --user --scope -p Delegate=yes` themselves to obtain the
delegated cgroup v2 subtree `linux` needs (`scripts/delegated-lane.sh` does the same). This adapter
does it once, in Substrate: it asks the user's systemd manager for a transient scope with
`Delegate=yes`, moves the exec's process tree into it, and serves `limits` (pids, memory+swap, CPU
from the scope's controllers) and whole-tree kill (`cgroup.kill`) to the adapter it wraps. Wrapping
`local` gives a bounded but unconfined run; wrapping `linux` gives `linux` its delegated root
without a caller-side `systemd-run`.

## Acceptance

- `confinement::open(systemd { inner: local })` on a host with a user systemd manager serves
  `limits`; a profile with `pids_max` 8 whose command forks 32 children observes the fork refusal,
  and a `wait` after a kill finds no process of the tree left (`cgroup.procs` empty).
- `confinement::open(systemd { inner: linux })` opens `linux` with the scope's cgroup as its
  delegated root, and the `linux` scenarios of the suite pass through it.
- Without a reachable user manager (`systemctl --user` fails, or `DBUS_SESSION_BUS_ADDRESS` unset)
  the open is refused `confinement.adapter-unserved`, and the `systemd` lane's cases are absent,
  never passed.
- `crates/substrate-host/tests/ess_confinement_systemd.rs` executes the `systemd` scenarios under
  `scripts/delegated-lane.sh`, and the lane's summary names the count.
- Adversary pass plus security-reviewer pass (confinement, cgroup delegation).

## Scope

`crates/substrate-host/src/confinement/systemd.rs` (new), `crates/substrate-host/src/confinement/mod.rs`
(registration), `crates/substrate-host/tests/ess_confinement_systemd.rs` (new),
`scripts/delegated-lane.sh`, `CHANGELOG.md`.

## Depends on

`story:confinement-contract-on-linux-and-local`.
