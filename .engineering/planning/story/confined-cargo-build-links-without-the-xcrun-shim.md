---
format: aep.planning-md/3
id: story:confined-cargo-build-links-without-the-xcrun-shim
kind: story
status: active
title: A confined cargo build links without the xcrun shim
relations:
- decomposes: epic:confinement-contract-family
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-10-27T07:24:37Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-27T07:24:38Z", actor: "human:timo", revision: 3}
---
## Outcome

A Seatbelt-confined `cargo build` or `cargo test` on macOS links without the test supplying a
linker: the macOS adapter, or the S2 policy's documented setup, resolves the toolchain `clang`
outside the sandbox so the `/usr/bin/cc` `xcrun` shim, which runs `/bin/sh` and `xcodebuild`, is
never reached.

## Origin

`decision-blocker:macos-s2-cargo-link-in-seatbelt`: the S2 cases pass only because
`crates/substrate-host/tests/seatbelt_adversary.rs` sets `RUSTC`, `SDKROOT` and the linker itself.

## Rejected

- Declaring `/bin` as an exec subpath: widens exec.
- Admitting `xcrun`'s per-user cache: a path under `/var/folders`, which the policy rules exclude.

## Acceptance

On macos-15, a fixture crate with a build script runs `cargo test` under the S2 policy with no
`RUSTC`, `SDKROOT` or linker set by the caller, and passes.
