---
format: aep.planning-md/3
id: story:standalone-docs-and-ess-adoption
kind: story
status: draft
title: Adopt ESS and a standalone Substrate documentation site
relations:
- informed_by: epic:resource-bounded-execution
scope:
- confidence: cited
  path: .engineering
- confidence: cited
  path: .github/workflows
- confidence: cited
  path: AGENTS.md
- confidence: cited
  path: README.md
- confidence: cited
  path: spec
- confidence: cited
  path: website
- confidence: cited
  path: xtask
revision: 2
---
## Outcome

The operator requested the current AEP Git-native store, ESS adoption for the existing implementation, and a standalone public documentation site matching Mantle's delivery approach, without integration into the organization-wide documentation site.

## Scope

Repository-local changes: `.engineering/`, `spec/`, `xtask/`, `website/`, documentation workflow callers, `AGENTS.md`, `README.md`, and gate entry points. Existing runtime semantics and immutable contract bundle bytes remain authoritative. New executable tooling is Rust with clap derive. No global Website or Atlas edits.

## Acceptance

- AEP-MIGRATE: migrate 119 artifacts to aep.project/5 using `--verify`, retaining states, revisions, relations, bodies, transitions and evidence.
- ESS-LEDGER: derive the durable operation lifecycle from the wire and SQLite store, validate and compile it, synthesize scenarios, and explicitly report the boundary covered and any compiler refusals. Conformance claims require execution against production code.
- DOCS-LOCAL: the Rust-built site retains public adopter guides, adds an overview and security boundaries, validates local links, and has `/substrate/` canonical routes and commit provenance.
- DOCS-DELIVERY: repository validation emits a main-commit artifact; the standalone project-site caller consumes that artifact. Remove repository-side unified docs bundle and redirect callers. Public output includes no planning store or internal design documents.
- CHECKS: run the repository's full gate and record actual outcomes, including unavailable lanes.

## Source observations

The original store was aep.project/1. `crates/substrate-store/src/operations.rs` owns operation reservation and completion. `crates/substrate-wire/src/lib.rs` declares OperationState and OperationRecord. `website/docs/` contains the existing public guides; the old `b10x.docs.yaml` and three `b10x-docs-*` callers selected centralized rendering and delivery.
