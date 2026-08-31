---
format: aep.planning-md/1
id: story:rust-sdk-publication
kind: story
status: draft
title: The Rust SDK and runtime chain are owner-released Rust packages
summary: An explicit allowlist, package gate and manual token-off-GitHub release make the SDK consumable without sibling paths.
owner: substrate
tags:
- release
- sdk
relations:
- decomposes: epic:rust-sdk
- depends_on: story:contract-bundle-oci-artifact
revision: 1
---
# Story: The Rust SDK and runtime chain are owner-released Rust packages

## Outcome

Consumers depend on version-locked `b10x-substrate-*` packages rather than a sibling checkout. The default client stays transport-only; linked daemon support is opt-in.

## Acceptance

The gate permits only the named package set, verifies package contents and downstream builds, and keeps every other workspace member non-publishable. A fully gated annotated release tag is published to crates.io manually in dependency order with an operator-held scoped token that never enters GitHub.
