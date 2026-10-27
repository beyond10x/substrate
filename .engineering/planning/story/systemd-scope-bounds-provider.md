---
format: aep.planning-md/3
id: story:systemd-scope-bounds-provider
kind: story
status: implemented
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
  path: crates/substrate-host/src/confinement/systemd.rs
- confidence: inferred
  path: crates/substrate-host/tests/ess_confinement_systemd.rs
- confidence: inferred
  path: scripts/delegated-lane.sh
revision: 7
transitions:
- {from: "draft", to: "proposed", at: "2026-10-27T01:13:04Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"review_outcome":2}}}
- {from: "proposed", to: "active", at: "2026-10-27T01:13:04Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"review_outcome":2}}}
- {from: "active", to: "implemented", at: "2026-10-27T07:47:38Z", actor: "human:timo", revision: 7, decided_on: {"recorded":{"test_result":1,"review_outcome":4}}}
---
# The `systemd` adapter: a delegated user scope as the bounds provider

ADR 0109 decision 3: `systemd` is a scope as the bounds provider. Today loom, takt, conductor and
metaharness each call `systemd-run --user --scope -p Delegate=yes` themselves to obtain the
delegated cgroup v2 subtree `linux` needs (`scripts/delegated-lane.sh` does the same). This adapter
does it once, in Substrate: it asks the user's systemd manager for a transient scope with
`Delegate=yes`, moves the exec's process tree into it, and serves `limits` (pids, memory+swap, CPU
from the scope's controllers) and whole-tree kill (`cgroup.kill`) to the adapter it wraps. Wrapping
`local` gives a bounded but unconfined run, still reported `declared_insecure`; wrapping `linux`
gives `linux` its delegated root without a caller-side `systemd-run`.

## Acceptance

- `confinement::open(systemd { inner: local })` on a host with a user systemd manager serves
  `limits`; a profile with `pids_max` 8 whose command forks 32 children observes the fork refusal,
  and a `wait` after a kill finds no process of the tree left (`cgroup.procs` empty).
- `confinement::open(systemd { inner: linux })` opens `linux` with the scope's cgroup as its
  delegated root, and the `linux` scenarios of the suite pass through it.
- Without a reachable user manager (`systemctl --user` fails, or `DBUS_SESSION_BUS_ADDRESS` unset)
  the open is refused `confinement.adapter-unserved`, and the `systemd` lane's cases are absent,
  never passed.
- `crates/substrate-host/tests/ess_confinement_systemd.rs` executes every `systemd` scenario of the
  suite (both `inner` variants) under `scripts/delegated-lane.sh`, and the lane's summary names the
  count.
- An `aep:adversary` pass and an `aep:security-reviewer` pass (cgroup delegation) are each recorded
  as a `review-result` that `reviews:` this story, and each of their findings has a
  `review_outcome` record: `fixed` in this story, or `escalated` with a filed story id.

## Scope

`crates/substrate-host/src/confinement/systemd.rs` (replaces the S0 stub),
`crates/substrate-host/tests/ess_confinement_systemd.rs` (new), `scripts/delegated-lane.sh`,
`CHANGELOG.md` (one line under `## [Unreleased]`; the other units of its wave add theirs, and the
merge into the integration branch keeps every line).

## Depends on

`story:confinement-contract-on-linux-and-local`.
