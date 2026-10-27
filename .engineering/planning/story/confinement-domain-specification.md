---
format: aep.planning-md/3
id: story:confinement-domain-specification
kind: story
status: implemented
title: The confinement contract is specified as the ESS domain substrate.confinement
owner: substrate
tags:
- adr-0109
relations:
- decomposes: epic:confinement-contract-family
- depends_on: story:ess-pin-0-70-0
scope:
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: adr/0044-confinement-is-a-contract-family-served-by-declared-adapters.md
- confidence: inferred
  path: adr/README.md
- confidence: inferred
  path: crates/substrate-host/src/generated/confinement.rs
- confidence: inferred
  path: crates/substrate-host/src/lib.rs
- confidence: inferred
  path: spec/README.md
- confidence: inferred
  path: spec/confinement
- confidence: inferred
  path: spec/confinement-suite.json
- confidence: inferred
  path: spec/coverage.md
- confidence: inferred
  path: xtask/src/ess.rs
revision: 9
transitions:
- {from: "draft", to: "proposed", at: "2026-10-27T01:13:03Z", actor: "human:timo", revision: 7, decided_on: {"recorded":{"review_outcome":4}}}
- {from: "proposed", to: "active", at: "2026-10-27T01:13:03Z", actor: "human:timo", revision: 8, decided_on: {"recorded":{"review_outcome":4}}}
- {from: "active", to: "implemented", at: "2026-10-27T07:47:38Z", actor: "human:timo", revision: 9, decided_on: {"recorded":{"test_result":1,"review_outcome":5}}}
---
# The confinement contract is specified as the ESS domain `substrate.confinement`

Atlas ADR 0109 decision 7 puts the contract in ESS under `spec/`, with one conformance scenario per
promise and adapter, and an absent fact as a refusal scenario, never a skip. Substrate invariant 8
requires a local ADR before code; this story writes Substrate ADR 0044 and the domain, and nothing
that runs a confined process. Every adapter's scenarios are authored here, so the adapter stories
add a test file each and never edit the committed suite.

## Model (the decisions this story takes; the domain is the authority once it validates)

- `substrate.confinement.ContractFamily` (enum): `process_run`, `process_watch`, `output_read`,
  `wait`, `workspace`, `filesystem_view`, `network_egress`, `network_ingress`, `limits`, `audit`
  (ADR 0109 decision 8, in that order).
- `substrate.confinement.AdapterName` (enum): `local`, `linux`, `macos`, `systemd`, `oci`, `pod`.
  `systemd` carries an `inner` adapter (`local` or `linux`): it is the bounds provider around one
  of them (ADR 0109 decision 3).
- `substrate.confinement.Fact` (enum): the facts an adapter proves at open and a profile may
  require: `declared_insecure`, `paths_are_host_paths`, `whole_tree_kill`, `local_permitted` (the
  host's policy fact, F3 A), `ipc_socket_allowlist` and `ipc_signals_contained` (ADR 0109 decision
  1's IPC: socket families, no signal outside the tree), `sealed_paths`, `exec_subpaths`,
  `private_tmpdir`, `host_network`, `image_digest_pinned`.
- Entity `Adapter`: identity `AdapterName`; fields `served_families: List<ContractFamily>` and
  `facts: List<Fact>`. Each adapter's sets are declared from the code that exists or the story that
  will build it, with an `UNMAPPED:` marker where neither says.
- Entity `Profile` and command `admit_profile`: input `adapter` (with `inner` for `systemd`),
  `required_families`, `required_facts`, `local_reason` (required when `adapter` is `local`),
  `image` (for `oci` and `pod`), and the reserved `ingress_routes` and `rate_bounds` (ADR 0109 F1 B).
  Outcomes: admitted (records the adapter, the families, the facts and, for `local`,
  `declared_insecure: true` with the reason); refused.
- Refusals (each an `errors:` entry with its code and status):
  `confinement.family-unserved` (501, names the family),
  `confinement.fact-unserved` (501, names the fact; ADR 0109 decision 2),
  `confinement.adapter-unserved` (501, the adapter cannot open on this host or platform),
  `confinement.local-refused` (403, the host's policy fact forbids `local`),
  `confinement.local-reason-missing` (422),
  `confinement.image-unpinned` (422, an `oci` or `pod` image named by tag, not by digest),
  `confinement.reserved-field-unserved` (501, an ingress route or a rate bound was set).
- Audit: every admission and every refusal emits an event (`ProfileAdmitted`, `ProfileRefused`)
  carrying the adapter and the family or fact named.

## Acceptance

- `ess specify validate --path spec/confinement --strict-requires` exits 0 with 0 warnings on the
  ESS release `story:ess-pin-0-70-0` pinned.
- `spec/confinement-suite.json` is synthesized from the domain plus authored scenarios under
  `spec/confinement/scenarios/`, and holds at least:
  - for each of the nine families other than `network_ingress`, one admission on an adapter that
    serves it; for each family that some adapter does not serve, one `confinement.family-unserved`
    refusal on such an adapter (families every adapter serves have none; `network_ingress` has a
    refusal and no admission);
  - per adapter, for each of `local`, `linux`, `macos`, `systemd { inner: local }`,
    `systemd { inner: linux }`, `oci` and `pod`: at least one admission and at least one
    `confinement.family-unserved` or `confinement.fact-unserved` refusal;
  - one `confinement.adapter-unserved` scenario for each of `linux`, `macos`, `systemd`, `oci` and
    `pod`, each with the setup that makes it absent: `macos` opened on Linux and `linux` opened on
    macOS; `systemd` with no user manager reachable (`DBUS_SESSION_BUS_ADDRESS` unset); `oci` with
    its runtime program set to a path that does not exist; `pod` with no cluster configuration.
    `local` always opens and has no such scenario; its absence case is `confinement.local-refused`;
  - every scenario names the lane that runs it as a tag (`portable`, `delegated`, `macos`, `oci`,
    `pod`) and the adapter in its name; each runner selects scenarios by lane tag, so the
    `adapter-unserved` scenarios run in the lane where the adapter is absent (`portable` for
    `macos`, `systemd`, `oci` and `pod`; `macos` for `linux`);
  - the `macos` adapter's facts are declared in their final state (after
    `story:macos-seatbelt-policy-runs-cargo-test`), and its scenarios are split by name:
    `macos-s1-*` (S1's adapter) and `macos-s2-*` (the `sealed_paths`, `exec_subpaths` and
    `private_tmpdir` admissions S2 makes pass);
  - `confinement.fact-unserved` for `macos` requiring `whole_tree_kill`, and for `linux` requiring
    `host_network`;
  - `local` admitted with a reason and reported `declared_insecure: true`; `local` without a reason
    refused `confinement.local-reason-missing`; `local` refused `confinement.local-refused` when
    `local_permitted` is false;
  - an `oci` image named by tag refused `confinement.image-unpinned`;
  - a set `ingress_routes` and a set `rate_bounds` each refused
    `confinement.reserved-field-unserved`.
- `cargo xtask check-ess` validates the domain, holds `spec/confinement-suite.json` to its
  regenerated bytes, runs it against the ESS `interpreted` reference (`--report-format 2`) with 0
  failures, and holds the generated Rust types `ContractFamily`, `AdapterName` and `Fact` in
  `crates/substrate-host/src/generated/confinement.rs` to their regenerated bytes; a hand edit to
  either file fails it.
- ADR 0044 "Confinement is a contract family served by declared adapters" is accepted, cites atlas
  ADR 0109 and ADR 0037, states that the `oci` and `pod` adapters serve the confinement contract
  through substrate-host's API and are not plan 03's wire-level drivers, is in `adr/README.md`, and
  `cargo xtask check-adrs` passes.
- `spec/README.md` and `spec/coverage.md` say the interpreted run checks the model, not an adapter.
- The executable-system-specification artifact `executable-system-specification:confinement` is
  created with the interpreted report recorded as evidence.
- `contracts/` and `crates/substrate-wire/` are unchanged by this story (`git diff --stat` of both
  against the story's base is empty).

## Scope

`spec/confinement/` (new: `system.yaml`, `ess-inputs.yaml`, `confinement.yaml`, `scenarios/`,
`README.md`), `spec/confinement-suite.json` (new), `xtask/src/ess.rs` (a `check_confinement` and its
call in `check`), `crates/substrate-host/src/generated/confinement.rs` (new) and its `mod` line in
`crates/substrate-host/src/lib.rs`, `adr/0044-*.md`, `adr/README.md`, `spec/README.md`,
`spec/coverage.md`, `CHANGELOG.md` (one line under `## [Unreleased]`).

## Depends on

`story:ess-pin-0-70-0`.

## Not in this story

Any adapter code; any wire or bundle change; the ESS pin move.
