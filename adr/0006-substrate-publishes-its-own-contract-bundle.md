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
