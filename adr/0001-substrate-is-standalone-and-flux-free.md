---
date: 2026-08-13
status: accepted
---

# ADR 0001: substrate is standalone and Flux-free

## Context

Flux already contains guarded local execution behavior and may become a major substrate client.
Depending on Flux would nevertheless reverse the foundation/product boundary, couple substrate's
release to a coding agent, and prevent other clients from adopting the execution contract cleanly.
The umbrella decision is recorded in
[daemonloom/architecture ADR 0004 — Substrate is standalone and Flux-free](https://github.com/daemonloom/daemonloom/blob/main/architecture/adr/0004-substrate-contract-is-flux-free.md).

## Decision

Substrate owns its wire, domain types, errors, capability facts, driver ports, guarded host
implementation, and sandbox enforcement. No Flux crate or type appears in any dependency kind,
public contract, or private adapter. Flux behavior may inform threat cases and conformance tests but
is neither copied as a source module nor used as an implementation dependency.

## Consequences

Flux may implement a client adapter over the released substrate protocol. Autodev, connectors,
agent, and other products can use the same service independently. Substrate can build, test,
release, and operate without a Flux checkout, binary, package, or protocol.
