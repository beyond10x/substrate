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
revision: 5
---
# Story: The Rust SDK and runtime chain are owner-released Rust packages

## Outcome

Consumers depend on version-locked `b10x-substrate-*` packages rather than a sibling checkout. The default client stays transport-only; linked daemon support is opt-in.

## Acceptance

The gate permits only the named package set, verifies package contents and downstream builds, and keeps every other workspace member non-publishable. A fully gated annotated release tag is published to crates.io manually in dependency order with an operator-held scoped token that never enters GitHub.

## Current state — 2026-09-01

The implementation, package allowlist, exact internal version edges, package-content checks, README material and public SDK journeys shipped on main and pass the required Full gate. This story stays active only for the external publication proof: publish the owner-released package chain to crates.io in dependency order from a fully gated annotated tag, verify anonymous clean-room installation, and record the registry versions and digests. No GitHub secret or repository-held crates.io token is introduced.
