---
format: aep.planning-md/3
id: story:sandbox-repository-parity
kind: story
status: implemented
title: The sandbox repository's settings are covered by Substrate
owner: substrate
tags:
- adr-0109
relations:
- decomposes: epic:confinement-contract-family
- depends_on: story:oci-adapter
- depends_on: story:confinement-contract-on-linux-and-local
- depends_on: story:confinement-consumer-surface
scope:
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: Cargo.lock
- confidence: inferred
  path: crates/b10x-substrate-cli/Cargo.toml
- confidence: inferred
  path: crates/b10x-substrate-cli/src
- confidence: inferred
  path: crates/substrate-host/tests/sandbox_parity.rs
- confidence: inferred
  path: spec/client/cli.yaml
- confidence: inferred
  path: spec/client/client.yaml
- confidence: inferred
  path: spec/generated/cli
revision: 8
transitions:
- {from: "draft", to: "proposed", at: "2026-10-27T01:13:04Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"review_outcome":4}}}
- {from: "proposed", to: "active", at: "2026-10-27T01:13:04Z", actor: "human:timo", revision: 7, decided_on: {"recorded":{"review_outcome":4}}}
- {from: "active", to: "implemented", at: "2026-10-27T07:47:38Z", actor: "human:timo", revision: 8, decided_on: {"recorded":{"test_result":1,"review_outcome":6}}}
---
# The sandbox repository's settings are covered by Substrate

ADR 0109 decision 5: `sandbox` is retired into Substrate; its bubblewrap settings are proved covered
by moving its mirror test (`sandbox` `tests/substrate_mirror.rs` at `1b0377d`) into Substrate, and
Atlas's O6 loop then calls Substrate's command line. This story does the Substrate half. Atlas's O6
loop keeps calling the `b10x-sandbox` binary (`atlas` `scripts/o6-loop.sh:67-83`) until its own hop;
archiving `sandbox` is the operator's act.

The argv comes from `ConfinementAdapter::render`, which `story:confinement-contract-on-linux-and-local`
(for `linux`) and `story:oci-adapter` (for `oci`) provide; this story reads it and does not edit
`crates/substrate-host/src/`.

## Acceptance

- `crates/substrate-host/tests/sandbox_parity.rs` holds the `sandbox` bubblewrap argument list
  (`src/confinement.rs:179-258`: `--unshare-user --disable-userns`, `--unshare-ipc`, `--unshare-pid`,
  `--unshare-uts`, `--unshare-net`, `--die-with-parent`, the read-only `/usr`, `/bin`, `/lib`,
  `/lib64`, `--proc`, `--dev`, `--tmpfs /tmp`, read-only roots, the `/workspace` tmpfs with binds
  and `--remount-ro`, `--chdir`, `--clearenv` and the five variables) as a table, and asserts each
  is present in the `linux` adapter's `render` output for the equivalent profile, so Substrate's
  set is a superset (it adds `--new-session` and `--seccomp`).
- The same file holds the `sandbox` Docker flags (`src/docker.rs:42-130`) and asserts each is
  present in the `oci` adapter's `render` output for the equivalent profile.
- `b10x-substrate confine run` takes `sandbox`'s flags (`--dir`, `--ro`, `--net`,
  `--backend linux|oci`, `--image`, `--dry-run`) plus `--local <reason>`, and runs the command
  through `confinement::open`. `--dry-run` prints the program and argv one per line, and the O6
  loop's own check (the recorded argv contains `--unshare-net`) holds for it.
- `confine run --backend linux --net` and `--backend oci --net` exit 2 before anything runs and
  print `confinement.fact-unserved: host_network`; with `--local <reason>` the command runs under
  `local` and the verb prints `adapter=local declared_insecure=<reason>` on stderr. Each is a CLI
  test case.
- `--image` takes only `<name>@sha256:<64 hex>`; a tag exits 2 and prints
  `confinement.image-unpinned` (CLI test case).
- The CLI's ESS binding (`spec/client/`) and its generated contract (`spec/generated/cli`) carry
  the verb, and `cargo xtask check-ess` passes.
- An `aep:adversary` pass and an `aep:security-reviewer` pass are each recorded as a
  `review-result` that `reviews:` this story, and each of their findings has a `review_outcome`
  record: `fixed` in this story, or `escalated` with a filed story id.

## Scope

`crates/substrate-host/tests/sandbox_parity.rs` (new), `crates/b10x-substrate-cli/src/` (the verb),
`crates/b10x-substrate-cli/Cargo.toml` and `Cargo.lock` (a path dependency on
`b10x-substrate-host` at the exact workspace version), `spec/client/client.yaml`,
`spec/client/cli.yaml`, `spec/generated/cli/`, `CHANGELOG.md` (one line under `## [Unreleased]`;
`story:macos-seatbelt-policy-runs-cargo-test` may add its line in the same wave, and the merge into
the integration branch keeps every line).

## Depends on

`story:confinement-consumer-surface` (the frozen surface it calls), `story:oci-adapter` (the
Docker half) and `story:confinement-contract-on-linux-and-local`.
