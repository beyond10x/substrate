---
format: aep.planning-md/3
id: story:confinement-contract-on-linux-and-local
kind: story
status: draft
title: 'S0: the confinement contract over the Linux driver, plus local'
owner: substrate
tags:
- adr-0109
relations:
- decomposes: epic:confinement-contract-family
- depends_on: story:confinement-domain-specification
scope:
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: crates/substrate-host/src/confinement/facts.rs
- confidence: inferred
  path: crates/substrate-host/src/confinement/linux.rs
- confidence: inferred
  path: crates/substrate-host/src/confinement/local.rs
- confidence: inferred
  path: crates/substrate-host/src/confinement/mod.rs
- confidence: inferred
  path: crates/substrate-host/src/confinement/profile.rs
- confidence: inferred
  path: crates/substrate-host/src/lib.rs
- confidence: inferred
  path: crates/substrate-host/tests/ess_confinement_linux.rs
- confidence: inferred
  path: crates/substrate-host/tests/ess_confinement_local.rs
- confidence: inferred
  path: crates/substrate-host/tests/support
- confidence: inferred
  path: scripts/delegated-lane.sh
revision: 2
---
# S0: the confinement contract over the Linux driver, plus `local`

ADR 0109's first slice. `b10x-substrate-host` gains `ConfinementProfile`, the `ConfinementAdapter`
trait and two adapters: `linux`, a wrapper over the existing `HostDriver`
(`crates/substrate-host/src/lib.rs:1040`), and `local`, an unconfined host-process runner declared
insecure. A client selects an adapter through substrate-host's API (`confinement::open`), not
through the wire: `HostDriverKind` (`crates/substrate-wire/src/lib.rs:3368`) and every bundle stay
unchanged.

## Contract (the shape; names follow the domain of `story:confinement-domain-specification`)

- `ConfinementProfile`: workspaces, filesystem view (read-only roots and files), network (none, or
  egress apertures), process bounds, I/O bounds, secret slots, the audit sink, and the reserved
  `ingress_routes` and `rate_bounds` (ADR 0109 F1 B).
- `ConfinementAdapter`: `facts()` (proved at open, including `served_families`), `admit(&profile)`
  (a refusal names the family or fact), `start`, `observe` (watch), `output`, `wait`, `signal`.
- `linux` derives its served families from the `CapabilitySnapshot` the probe proved; a family
  whose proving fact is absent is not served. `network_ingress` is served by no adapter.
- `local` serves `process_run`, `process_watch`, `output_read`, `wait`, `workspace` (a host
  directory) and `audit`, and nothing that would claim confinement: a profile under `local` that
  names a filesystem view, a network policy or limits is refused `confinement.family-unserved`.
  `local` is opened only by name with a non-empty reason, and every observation carries
  `adapter: local, declared_insecure: <reason>`. The host's policy fact refuses it
  (`confinement.local-refused`) when set (F3 A). Nothing falls back to `local`.

## Acceptance

- `crates/substrate-host/tests/ess_confinement_local.rs` executes every `local` scenario of the
  committed `spec/confinement-suite.json` against the real `local` adapter, pins the count, and
  fails on an unknown step; it runs in `cargo test --workspace --release --locked` (portable lane).
- `crates/substrate-host/tests/ess_confinement_linux.rs` executes every `linux` scenario against
  the real adapter over `HostDriver`; it runs under `scripts/delegated-lane.sh` and is absent, never
  passed, without `SUBSTRATE_VECTORS_CGROUP_ROOT`; the delegated lane's own summary names its case
  count.
- Under `linux`, a profile needing `limits` on a host without `exec_cgroup_limits` is refused
  `confinement.family-unserved` naming `limits` (portable lane case).
- No wire change: `cargo xtask check-bundles 0.5.0 … 0.23.0` and the runtime vectors
  (`PORTABLE_CASES` 118, `DELEGATED_CASES` 180) are unchanged.
- An adversary pass (confinement) and a security-reviewer pass find no open defect, or each finding
  is a fix in this story or a filed story.

## Scope

`crates/substrate-host/src/confinement/` (new: `mod.rs`, `profile.rs`, `facts.rs`, `linux.rs`,
`local.rs`), `crates/substrate-host/src/lib.rs` (exports), `crates/substrate-host/tests/support/`
(shared suite runner, new), `crates/substrate-host/tests/ess_confinement_local.rs`,
`crates/substrate-host/tests/ess_confinement_linux.rs`, `scripts/delegated-lane.sh`,
`CHANGELOG.md`.

## Depends on

`story:confinement-domain-specification` (the suite and generated types).
