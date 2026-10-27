---
format: aep.planning-md/3
id: story:confinement-contract-on-linux-and-local
kind: story
status: implemented
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
  path: crates/substrate-host/src/confinement/macos.rs
- confidence: inferred
  path: crates/substrate-host/src/confinement/mod.rs
- confidence: inferred
  path: crates/substrate-host/src/confinement/oci.rs
- confidence: inferred
  path: crates/substrate-host/src/confinement/pod.rs
- confidence: inferred
  path: crates/substrate-host/src/confinement/profile.rs
- confidence: inferred
  path: crates/substrate-host/src/confinement/systemd.rs
- confidence: inferred
  path: crates/substrate-host/src/lib.rs
- confidence: inferred
  path: crates/substrate-host/src/process.rs
- confidence: inferred
  path: crates/substrate-host/tests/ess_confinement_linux.rs
- confidence: inferred
  path: crates/substrate-host/tests/ess_confinement_local.rs
- confidence: inferred
  path: crates/substrate-host/tests/support
- confidence: inferred
  path: scripts/delegated-lane.sh
revision: 8
transitions:
- {from: "draft", to: "proposed", at: "2026-10-27T01:13:03Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"review_outcome":2}}}
- {from: "proposed", to: "active", at: "2026-10-27T01:13:03Z", actor: "human:timo", revision: 7, decided_on: {"recorded":{"review_outcome":2}}}
- {from: "active", to: "implemented", at: "2026-10-27T07:47:38Z", actor: "human:timo", revision: 8, decided_on: {"recorded":{"test_result":1,"review_outcome":5}}}
---
# S0: the confinement contract over the Linux driver, plus `local`

ADR 0109's first slice. `b10x-substrate-host` gains `ConfinementProfile`, the `ConfinementAdapter`
trait and two adapters: `linux`, a wrapper over the existing `HostDriver`
(`crates/substrate-host/src/lib.rs:1040`), and `local`, an unconfined host-process runner declared
insecure. A client selects an adapter through substrate-host's API (`confinement::open`), not
through the wire: `HostDriverKind` (`crates/substrate-wire/src/lib.rs:3368`) and every bundle stay
unchanged. This story owns the public surface of the `confinement` module and the adapter
registry; the adapter stories after it fill one file each and do not edit `mod.rs`.

## Contract (the shape; names follow the domain of `story:confinement-domain-specification`)

- `ConfinementProfile`: workspaces (named writable subtrees), declared writable host roots outside
  the workspace (ADR 0038), a working directory beneath a workspace or root (design 26),
  filesystem view (read-only roots and files), sealed paths inside a workspace, exec subpaths, a private `TMPDIR`, network (none, or
  egress apertures), IPC (allowed socket families; signals stay inside the tree), process bounds,
  I/O bounds, secret slots, the audit sink, an image digest (for `oci` and `pod`), and the reserved
  `ingress_routes` and `rate_bounds` (ADR 0109 F1 B). Each field maps to the family or fact it
  requires; an adapter that does not serve it refuses it by that name.
- `ConfinementAdapter`: `facts()` (proved at open: `served_families` and `facts`),
  `admit(&profile)` (a refusal names the family or fact), `render(&admitted, &command)` (the program
  and argv the adapter would run, for adapters that run one; used by `--dry-run` and the parity
  test), `start`, `observe` (watch), `output`, `wait`, `signal`.
- Composition: `open` takes an `AdapterRequest`; `Systemd { inner }` holds the request of the
  adapter it wraps (`Local { reason }` or `Linux`), and `Linux` takes an optional delegated cgroup
  root that becomes `HostConfig.cgroup_root` (`crates/substrate-host/src/lib.rs:268`). `systemd.rs`
  composes through these two and edits neither `linux.rs` nor `mod.rs`.
- The registry in `confinement/mod.rs` names all six adapters. `macos.rs`, `systemd.rs`, `oci.rs`
  and `pod.rs` are created here as stubs whose `open` returns `confinement.adapter-unserved`; each
  adapter story replaces only its own file.
- `linux` derives its served families and facts from the `CapabilitySnapshot` the probe proved; a
  family whose proving fact is absent is not served. It proves `ipc_socket_allowlist` from the
  seccomp socket filter (ADR 0039) and `ipc_signals_contained` from its pid namespace;
  `sealed_paths` and `host_network` are not proved and are refused `confinement.fact-unserved`.
  `network_ingress` is served by no adapter.
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
- `confinement::open` for `macos`, `systemd`, `oci` and `pod` on this tree returns
  `confinement.adapter-unserved` (portable lane, one case each).
- `render` under `linux` for a fixed profile returns a `bwrap` argv that contains
  `--unshare-net`, `--new-session` and `--die-with-parent` (portable lane).
- No wire change: `cargo xtask check-bundles 0.5.0 … 0.23.0` passes, `contracts/` and
  `crates/substrate-wire/` are unchanged, and the runtime vectors (`PORTABLE_CASES` 118,
  `DELEGATED_CASES` 180) are unchanged.
- An `aep:adversary` pass and an `aep:security-reviewer` pass are each recorded as a
  `review-result` that `reviews:` this story, and each of their findings has a `review_outcome`
  record: `fixed` in this story, or `escalated` with a filed story id.

## Scope

`crates/substrate-host/src/confinement/` (new: `mod.rs`, `profile.rs`, `facts.rs`, `linux.rs`,
`local.rs`, and the stubs `macos.rs`, `systemd.rs`, `oci.rs`, `pod.rs`),
`crates/substrate-host/src/lib.rs` (exports), `crates/substrate-host/src/process.rs` (the argv
builder made callable by `render`, no behaviour change), `crates/substrate-host/tests/support/`
(shared suite runner, new), `crates/substrate-host/tests/ess_confinement_local.rs`,
`crates/substrate-host/tests/ess_confinement_linux.rs`, `scripts/delegated-lane.sh`,
`CHANGELOG.md` (one line under `## [Unreleased]`).

## Depends on

`story:confinement-domain-specification` (the suite and generated types).
