---
format: aep.planning-md/1
id: story:rust-sdk-publication
kind: story
status: active
title: The Rust SDK and runtime chain are owner-released Rust packages
summary: An explicit allowlist, package gate and manual token-off-GitHub release make the SDK consumable without sibling paths.
owner: substrate
tags:
- release
- sdk
- wave/remote-foundation-01
relations:
- decomposes: epic:rust-sdk
- depends_on: story:contract-bundle-oci-artifact
revision: 6
---
# Story: The Rust SDK and runtime chain are owner-released Rust packages

## Outcome

Consumers depend on version-locked `b10x-substrate-*` packages rather than a sibling checkout. The default client stays transport-only; linked daemon support is opt-in.

## Acceptance

The gate permits only the named package set, verifies package contents and downstream builds, and keeps every other workspace member non-publishable. A fully gated annotated release tag is published to crates.io manually in dependency order with an operator-held scoped token that never enters GitHub.

## Current state — 2026-09-01

The implementation, package allowlist, exact internal version edges, package-content checks, README material and public SDK journeys shipped on main and pass the required Full gate. This story stays active only for the external publication proof: publish the owner-released package chain to crates.io in dependency order from a fully gated annotated tag, verify anonymous clean-room installation, and record the registry versions and digests. No GitHub secret or repository-held crates.io token is introduced.

## Publication readiness audit — 2026-09-01

- Annotated tag `0.4.1` resolves to fully gated commit `961be39c34ef1f9b655a0b5a58a108dabc722d5c`; current main changes no package inputs after that tag.
- Anonymous crates.io API and search returned no package for all five approved names. This is current availability, not a reservation; first-publication races remain possible.
- Publish from a clean detached `0.4.1` checkout in dependency order: `b10x-substrate-wire`, `b10x-substrate-store`, `b10x-substrate-host`, `b10x-substrate-daemon`, `b10x-substrate-sdk`.
- Two package runs were byte-identical. Predicted registry checksums are wire `c622fcee231eb2fe7c238b122642b3e9b2734d2d0023e2d1e0e4d2bf3c710c88`, store `fe785c9fd580c40ec8f207fa2edc19c9543bf8bc9c9a802ffb9ca5ffd1bffd10`, host `9aefc892172da3d348953b93dd07ada7d2deb43e13010e66937c66e4b9bdd16e`, daemon `f134c26509711060c6257ecd17e04101116f23571737c595021e01f396299866`, SDK `e45d37f8eb28f10b39c1f910f6715d467f90736108e904e781f59031f527adab`.
- Temporary-registry dependency resolution and SDK `linked-daemon` verification passed. The owner-private Cargo credentials file exists, but token validity and `publish-new` scope remain intentionally unproven until the operator-authorized live publication.
- Closure still requires the contract-bundle dependency, five live registry checksums, and credential-free clean-room default SDK, linked-daemon and daemon installation tests.
