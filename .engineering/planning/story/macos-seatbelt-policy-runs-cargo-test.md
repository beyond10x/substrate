---
format: aep.planning-md/3
id: story:macos-seatbelt-policy-runs-cargo-test
kind: story
status: active
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
  path: crates/substrate-host/tests/ess_confinement_macos.rs
- confidence: inferred
  path: crates/substrate-host/tests/fixtures
- confidence: inferred
  path: crates/substrate-host/tests/seatbelt_adversary.rs
revision: 7
transitions:
- {from: "draft", to: "proposed", at: "2026-10-27T01:13:04Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"review_outcome":5}}}
- {from: "proposed", to: "active", at: "2026-10-27T01:13:04Z", actor: "human:timo", revision: 7, decided_on: {"recorded":{"review_outcome":5}}}
---
# S2: a Seatbelt policy that runs `cargo test`

ADR 0109 S2; chain-scope § 4 lists what the S1 policy lacks for a Rust build: writable subtrees
instead of the whole workspace, sealed paths inside it, exec by subpath (declared roots, writable
subtrees, system and developer directories), reads of the Xcode or CommandLineTools root and
`/private/var/db/xcode_select_link`, and a private per-exec `TMPDIR`. This is what takt's macOS
`(allow default)` profile does today with the network open; Substrate's stays `(deny default)` with
no network.

These are fields of the shared `ConfinementProfile` and facts of the domain, both declared by
`story:confinement-contract-on-linux-and-local` and `story:confinement-domain-specification`
(`workspaces`, `sealed_paths`, `exec_subpaths`, `private_tmpdir`). This story makes the `macos`
adapter prove the three facts at open and enforce them in `SeatbeltPolicy`; it changes neither the
domain nor `profile.rs`. A consumer reaches them by requiring the facts, never by naming `macos`.

## Acceptance

- After this story the `macos` adapter's facts include `sealed_paths`, `exec_subpaths` and
  `private_tmpdir`. Before it, `ess_confinement_macos.rs` asserts they are absent and refused
  `confinement.fact-unserved` (`story:macos-adapter-on-seatbelt`); this story flips that assertion
  in the same hunk, with this story's id in its message, and the test additionally executes every
  `macos-s2-*` scenario of the suite on the `macos-15` job with its pinned count raised.
- On the `macos-15` job, a new adversary case runs `cargo test --offline` of a fixture crate under
  the `macos` adapter with a vendored dependency and a build script, and asserts the inner
  `test result: ok`.
- A second case shows a write to a sealed path inside a writable subtree and an exec from an
  undeclared subpath are both refused.
- A third case observes the private `TMPDIR`: the child's `$TMPDIR` is a per-exec directory that
  is not the host's `TMPDIR`, a file written there is not visible to a second exec, and the
  directory is gone after `wait`.
- A fourth case passes a sealed path containing a double quote and gets
  `exec.sandbox-path-unrepresentable` before anything runs.
- A network connection from the fixture's test is refused (the existing outbound case, repeated
  under the build policy).
- The S1 scenarios and the seven earlier adversary cases still pass on the same job; the green run
  of the unit's final head is recorded as `test_result` evidence with `--ref <run URL>`.
- An `aep:adversary` pass and an `aep:security-reviewer` pass are each recorded as a
  `review-result` that `reviews:` this story, and each of their findings has a `review_outcome`
  record: `fixed` in this story, or `escalated` with a filed story id.

## Scope

`crates/substrate-host/src/seatbelt.rs`, `crates/substrate-host/src/confinement/macos.rs`,
`crates/substrate-host/tests/seatbelt_adversary.rs`, `crates/substrate-host/tests/ess_confinement_macos.rs`,
`crates/substrate-host/tests/fixtures/`
(a small fixture crate, new), `.github/workflows/macos.yml`, `CHANGELOG.md` (one line under
`## [Unreleased]`; the other units of its wave add theirs, and the merge into the integration
branch keeps every line).

## Depends on

`story:macos-adapter-on-seatbelt`.
