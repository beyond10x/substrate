---
format: aep.planning-md/1
id: story:pinned-rust-toolchain
kind: story
status: draft
title: The Rust toolchain is pinned so local and CI clippy agree
summary: rust-toolchain.toml pins 1.97, matching Cargo.toml rust-version and the Dockerfile builder; retires the 'rustup update before pushing' rule.
owner: substrate
tags:
- build
relations:
- decomposes: epic:release-hardening
revision: 2
---
# Story: The Rust toolchain is pinned so local and CI clippy agree

## Outcome

A commit that passes `cargo clippy -D warnings` locally passes it in CI, because both use the same
compiler. The `rustup update` rule in `AGENTS.md` is retired.

## Context

No `rust-toolchain.toml` exists. `AGENTS.md:116-119` documents the drift — "CI installs whatever
`stable` is that day, and a newer clippy can fail a commit that passed locally" — and asks people
to `rustup update` instead of removing the cause. The number is already agreed in two places:
`rust-version = "1.97"` (`Cargo.toml:14`) and `rust:1.97-bookworm` (`Dockerfile:2`).

## Acceptance

`rust-toolchain.toml` pins `1.97` with `rustfmt` and `clippy`, a gate step fails when that number,
`Cargo.toml` `rust-version` and the `Dockerfile` builder tag disagree, and `AGENTS.md` § *The gate*
says a bump is one commit that changes all three.

Evidence that satisfies it:

- `rust-toolchain.toml` at the root: `channel = "1.97"`, `components = ["rustfmt", "clippy"]`;
- `gate.yml` (`story:ci-runs-the-full-gate`) reads the toolchain from the file, not a channel name;
- the agreement check in `scripts/`, in `scripts/gate.sh`; verified failing-first by setting one of
  the three to `1.98`;
- `bash scripts/gate.sh` exits 0 on the pinned toolchain.
