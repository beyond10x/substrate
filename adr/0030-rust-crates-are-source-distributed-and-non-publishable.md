---
status: accepted
date: 2026-09-01
---

# ADR 0030: Rust crates are source-distributed and non-publishable

## Context

ADR 0022 approved a manual crates.io release for the runtime chain and SDK. No b10x Substrate
crate was ever published there, and the operator has clarified that b10x does not generally use
crates.io as a distribution surface. crates.io publication is permanent, reserves globally shared
names, and creates a second release authority beside the repository's signed GitHub and GHCR
artifacts. None of that reach is needed for current consumers: the repository is public under
Apache-2.0, Cargo can consume an exact Git revision or a checked-out path, and deployed services
consume signed native or OCI artifacts.

## Decision

Every Substrate workspace package sets `publish = false`, including the wire, store, host, daemon,
SDK and MCP packages. `cargo xtask check-packages` fails closed if any workspace member becomes
publishable. For the five runtime packages it additionally retains the source-consumption
guarantees that matter: fixed `b10x-substrate-*` names, exact internal version edges, an inherited
Apache-2.0 SPDX declaration, a checked-in README, and a public Substrate documentation target.

Rust consumers use a path dependency while developing beside Substrate or an exact Git revision in
a shared build. They do not use an unpinned branch. Deployments use the signed daemon and MCP OCI
images where a source dependency is not appropriate. Release tags continue to publish GitHub and
GHCR artifacts only; no crates.io credential, upload step, package checksum, or docs.rs claim is
part of a Substrate release.

This replaces only ADR 0022's registry-publication decision and the corresponding section of design
18. The SDK remains a wire client, linked mode remains a re-executed child, internal dependency
versions remain exact, and the development wire contract gains no stability promise.

## Consequences

Accidental `cargo publish` fails locally before contacting a registry. A consumer that wants Rust
source must pin the repository revision and therefore receives the matching internal crate graph as
one unit. Substrate does not reserve its crate names on crates.io and makes no promise that an
unrelated future owner of those names represents this project.

The abandoned publication story is archived with this finding rather than falsely marked
implemented. Reintroducing any registry publication requires a new accepted decision and a new
release/security review before a manifest can become publishable.
