---
format: aep.planning-md/3
id: story:release-publishes-static-musl-archives
kind: story
status: active
title: A release publishes static musl archives of the daemon and the CLI
summary: x86_64-unknown-linux-musl archives beside the gnu ones, static and checked in Alpine; mimalloc only where measured not slower
scope:
- confidence: inferred
  path: .github/workflows/release-lanes.yml
- confidence: cited
  path: .github/workflows/release.yml
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: Cargo.lock
- confidence: inferred
  path: Cargo.toml
- confidence: cited
  path: Dockerfile
- confidence: cited
  path: README.md
- confidence: inferred
  path: THIRD_PARTY_LICENSES.html
- confidence: inferred
  path: crates/b10x-substrate-cli/Cargo.toml
- confidence: inferred
  path: crates/b10x-substrate-cli/src/main.rs
- confidence: inferred
  path: crates/substrate-daemon/Cargo.toml
- confidence: inferred
  path: crates/substrate-daemon/src/main.rs
- confidence: inferred
  path: xtask/tests/release_workflow.rs
revision: 16
transitions:
- {from: "draft", to: "proposed", at: "2026-11-26T07:55:49Z", actor: "human:operator", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-26T07:55:50Z", actor: "human:operator", revision: 3}
---
# Story: a release publishes static musl archives of the daemon and the CLI

## Outcome

Every release attaches `substrate-daemon-<version>-x86_64-unknown-linux-musl.tar.gz` and
`b10x-substrate-<version>-x86_64-unknown-linux-musl.tar.gz` beside the two `gnu` archives, each
holding one statically linked executable that starts on any x86_64 Linux whatever its C library.
`SHA256SUMS` and its keyless signature cover all four archives. The `gnu` archives, their names
and their checks stay as they are.

## Where the requirement comes from

The repository owner's decision of 2026-11-25, option A: static musl Linux releases for every
beyond10x Rust tool, with mimalloc where a tool is allocation-heavy, and consumers pin the musl
archives. Reason given: a build host of the collection runs glibc 2.34, where a `gnu` binary
built against a newer glibc warns or fails. Stakeholder: the repository owner.

## What is there today

- `Dockerfile`'s `release-binaries` stage exports `substrate-daemon` and `b10x-substrate` for
  `x86_64-unknown-linux-gnu`; `release.yml` refuses a binary that needs a glibc newer than 2.35
  or a shared library outside the documented set, and starts both in a digest-pinned Ubuntu
  22.04 image.
- The executor image already builds `substrate-executor` for `x86_64-unknown-linux-musl` with
  `musl-tools` in the same builder, and `rust-toolchain.toml` lists that target.
- The daemon links OpenSSL through the host driver's Git stack (`gix` with
  `blocking-http-transport-curl-openssl`, `curl` with `static-curl`, `git2`) and `aws-lc-rs`
  through `rustls`. A static musl build needs each of them built for musl and linked statically.

## What changes

1. The `release-binaries` stage also builds both binaries for `x86_64-unknown-linux-musl`,
   statically linked, from the same pinned builder.
2. `release.yml` packages, checksums and signs the two musl archives with the `gnu` ones, and
   refuses a musl binary that has a `PT_INTERP` or a dynamic section naming a shared library, or
   that does not answer `--version` (or `--help` where it has no `--version`) in a plain Alpine
   container pinned by digest.
3. A check of the gate's or a lane's workflow builds both musl binaries and runs the same two
   checks on every pull request that changes what they build, so a musl break is found before a
   tag.
4. mimalloc is the global allocator of a binary only where a measurement on a workstation,
   recorded in the planning store, shows the musl binary with it is not slower than without it on
   that binary's allocation-heavy path (the daemon: a Git materialization and a workspace
   listing). Otherwise the binary keeps musl's allocator, and the story says so.
5. `README.md` and the release notes name the musl archives and say that they run on any x86_64
   Linux.

## Acceptance

A release dry run of the workflow (or the pull request check of item 3) produces both musl
archives; `readelf` shows each executable with no `PT_INTERP` and no `NEEDED` entry; each answers
`--version` in an Alpine container; `SHA256SUMS` lists four archives. The `gnu` checks pass
unchanged.

## Boundaries

- No change to the images: the daemon and MCP images keep their current base and binary.
- No new target beyond `x86_64-unknown-linux-musl`.
- No release is cut by this story.
