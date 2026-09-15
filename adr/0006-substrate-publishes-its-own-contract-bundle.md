---
status: accepted
date: 2026-08-13
---

# ADR 0006: substrate publishes its own contract bundle

## Context

Substrate's native operation vocabulary and connectors' catalog vocabulary intentionally differ.
Calling one a mechanical schema projection of the other was false and left no reproducible wire
authority for clients or drivers.

## Decision

Substrate owns the `substrate-wire` schema/conformance bundle defined by Design 07 and architecture
ADR 0019. Connectors owns a total, versioned projection manifest from a pinned substrate bundle to
its distinct catalog schema. Risk has conservative floors, idempotency has an explicit mapping,
semantic effects/auth/credentials are connectors-owned entries, and model exposure is translated
without treating direct channels as unary calls.

The generated provider document must match the catalog artifact byte-for-byte. Neither repository
uses the other's checkout or calls the schemas identical.

## Consequences

- The first implementation deliverable is the development bundle plus clean-room vectors.
- Connector projection is phase-6 adoption and does not enter substrate's runtime dependency graph.
- Repository-authored schema portions are marked as such while standard inputs retain provenance.

## Addendum — 2026-09-15: why a development bundle is signed, verified and write-once

This addendum changes no decision above. It records, in the ADR that owns the bundle, an answer a
review asked for: why keyless signing, digest pinning and write-once tags are applied to an artifact
every part of which is annotated `development` (102 files under `contracts/` carry that word; 82
files under `contracts/` and `scripts/` carry `signature`, `immutable` or `sealed`).

**What the release actually does.** It packages the explicitly pinned current bundle version into a
deterministic OCI layout, asserts the manifest annotation `dev.b10x.contract.status=development`,
publishes it as `ghcr.io/beyond10x/b10x-substrate-wire:<bundle-version>`, keyless-signs that digest
with cosign and verifies the signature against the exact tagged workflow identity **before** the
GitHub release exists — [`.github/workflows/release.yml`](../.github/workflows/release.yml), steps
*Package and inspect the development contract bundle*, *Publish or reuse the exact development
contract-bundle layout*, *Sign the development contract bundle keylessly* and *Verify the
contract-bundle signature with this release identity*. The bundle tag is write-once: an existing tag
whose digest differs from the deterministic local package refuses the release, and a byte-identical
digest is reused and signed again rather than replaced (*Resolve the write-once contract-bundle
tag*).

**Why it is signed, and where that was decided.** Not per release, and not here first.
[Design 07](../docs/design/07-specification-and-conformance.md) § 1 records that the bundle "follows
deterministic OCI packaging, signing, digest pinning, and clean-room conformance from ADR 0019" —
atlas ADR 0019, the cross-repository contract-release decision this ADR already defers to. That
decision counts a verified keyless signature over an anonymously retrievable immutable digest,
carrying the development annotation, among the evidence a bundle must **already** hold before any
promotion can be considered. The evidence is therefore collected while the bytes are development,
because frozen bytes distributed without it cannot acquire it afterwards. `story:contract-bundle-oci-artifact`
gives the consumer-side reason for the artifact itself — "A consumer pins the wire contract by OCI
digest instead of copying a directory tree" — and a signature is what makes that pin worth more than
the copy it replaced.

The same discipline is already recorded for another development-only artifact:
[ADR 0025](0025-the-mcp-adapter-is-a-disposable-test-surface.md) publishes the disposable MCP image
"write-once, signed by digest and verified before announcement under the same release discipline as
the daemon". One discipline over everything this repository publishes, rather than a per-artifact
judgement about which bytes deserve provenance.

**What it does not mean.** Nothing above promises stability. [`SECURITY.md`](../SECURITY.md)
§ *Supported line*: "A development bundle remains a development contract even when the daemon image
carrying it is signed." [`AGENTS.md`](../AGENTS.md) § *Releases*, [`README.md`](../README.md)
§ *Status* and the release notes say the same, and
[`xtask/tests/release_workflow.rs`](../xtask/tests/release_workflow.rs)
(`development_status_is_verified_and_never_described_as_stable`) fails if the workflow drifts.
Signing, digest pinning and a fixed tag answer provenance and distribution integrity — are these the
bytes this project published, and do they still live where it said — which is a different question
from compatibility, and answering the first does not begin to answer the second.

**Not recorded.** Why the *tag* is write-once rather than movable, for a bundle that carries no
stability promise, is stated nowhere as a reason: `release.yml` states the rule and refuses
accordingly, and no accepted text gives its motive. That part is recorded here as an observation,
not as a rationale, rather than inventing one after the fact. Atlas ADR 0019's requirement is the
immutability of the *digest*, which the write-once tag is sufficient but not necessary for.
