---
format: aep.planning-md/3
id: story:macos-seatbelt-policy-runs-cargo-test
kind: story
status: draft
title: 'S2: a Seatbelt policy that runs cargo test'
owner: substrate
tags:
- adr-0109
relations:
- decomposes: epic:confinement-contract-family
- depends_on: story:macos-adapter-on-seatbelt
scope:
- confidence: inferred
  path: .github/workflows/macos.yml
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: crates/substrate-host/src/confinement/macos.rs
- confidence: inferred
  path: crates/substrate-host/src/seatbelt.rs
- confidence: inferred
  path: crates/substrate-host/tests/fixtures
- confidence: inferred
  path: crates/substrate-host/tests/seatbelt_adversary.rs
revision: 2
---
# S2: a Seatbelt policy that runs `cargo test`

ADR 0109 S2; chain-scope § 4 lists what the S1 policy lacks for a Rust build: writable subtrees
instead of the whole workspace, sealed paths inside it, exec by subpath (declared roots, writable
subtrees, system and developer directories), reads of the Xcode or CommandLineTools root and
`/private/var/db/xcode_select_link`, and a private per-exec `TMPDIR`. This is what takt's macOS
`(allow default)` profile does today with the network open; Substrate's stays `(deny default)` with
no network.

## Acceptance

- `SeatbeltPolicy` gains writable subtrees, sealed paths beneath them, executable subpaths and a
  private `TMPDIR`; each is a profile field of the `macos` adapter, refused by name when its path is
  unrepresentable (`exec.sandbox-path-unrepresentable`).
- On the `macos-15` job, a new adversary case runs `cargo test --offline` of a fixture crate under
  the `macos` adapter with a vendored dependency and a build script, and asserts the inner
  `test result: ok`; a second case shows a write to a sealed path inside a writable subtree and an
  exec from an undeclared subpath are refused.
- A network connection from the fixture's test is refused (the existing outbound case, repeated
  under the build policy).
- The S1 scenarios and the seven earlier adversary cases still pass on the same job.
- Adversary pass plus security-reviewer pass (confinement).

## Scope

`crates/substrate-host/src/seatbelt.rs`, `crates/substrate-host/src/confinement/macos.rs`,
`crates/substrate-host/tests/seatbelt_adversary.rs`, `crates/substrate-host/tests/fixtures/`
(a small fixture crate, new), `.github/workflows/macos.yml`, `CHANGELOG.md`.

## Depends on

`story:macos-adapter-on-seatbelt`.
