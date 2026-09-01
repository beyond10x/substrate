---
format: aep.planning-md/1
id: story:promote-development-contract-frontier
kind: story
status: active
title: The daemon advertises the implemented contract frontier
summary: A clean-room consumer pins the current additive bundle before the daemon advances x-b10x-contract from 0.4.0.
owner: substrate
tags:
- contract
- release
- remote
- wave/remote-foundation-01
relations:
- decomposes: epic:release-hardening
- depends_on: story:confinement-runtime-hardening
- depends_on: story:aperture-install-failure-loses-its-errno
- depends_on: story:contract-bundle-oci-artifact
revision: 5
---
# Story: The daemon advertises the implemented contract frontier

## Outcome

The daemon advertises the newest fully implemented, gated additive contract bundle instead of substrate-wire/0.4.0, and a consumer can pin that claim without learning repository internals.

## Context

Development bundles through 0.11.0 are already implemented while the header remains 0.4.0. The substrate-hardening worktree is preparing an additional 0.12.0 scoped-workspace-access successor and SDK support. This story selects the actual current successor when its dependencies merge; it does not assume an unmerged bundle number.

## Acceptance

Before the header moves, an ADR or accepted design names the affected consumers and migration order. Every advertised operation, schema, refusal, fact and executable vector passes the shipped-binary clean-room lane. A clean-room Rust SDK fixture pins the new claim and rejects both an older and an unknown claim. The header changes atomically with release notes and consumer notification. Frozen bundles remain byte-identical, and the promoted bundle is still described as development unless a separate stability decision says otherwise.

## Out of Scope

New capability behavior, contract stability, protocol identifier renames and a generic version-negotiation protocol.

## Implementation evidence — 2026-09-01

- Atlas ADR 0019 names the daemon, Rust SDK and Harness migration order and authorises the exact `0.11.0`-to-`0.12.0` lineage bridge.
- The wire crate owns one atomic advertised name/digest pair; daemon and SDK consume it, and tests hash the frozen inner `contracts/substrate-wire/0.12.0/bundle.json` bytes.
- `cargo xtask check-bundle 0.12.0` now compares all 33 route declarations with `0.11.0` and re-runs the quota/metrics acceptance inventory. Mutation tests prove changed route authority and removed behavior are refused.
- The shipped-binary clean-room runner probes every declared HTTP and WebSocket operation, asserts the promoted pair on every response or upgrade, and completed 68 portable cases plus 95 delegated cases.
- The SDK accepts only the exact promoted pair and refuses missing, older, unknown and wrong-digest claims before operation-body decoding.
- `bash scripts/gate.sh`, `bash scripts/delegated-lane.sh`, `npm run typecheck` and `npm run build` passed in the implementation worktree.
- Public release notes and website pages distinguish the inner contract digest from the outer signed OCI digest and continue to label the bundle development.
- Harness compatibility observation remains required before this story moves from active to implemented.
