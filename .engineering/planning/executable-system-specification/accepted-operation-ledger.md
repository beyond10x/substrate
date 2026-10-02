---
format: aep.planning-md/3
id: executable-system-specification:accepted-operation-ledger
kind: executable-system-specification
status: draft
title: Accepted-operation ledger ESS projection
relations:
- specifies: story:standalone-docs-and-ess-adoption
revision: 1
---
## Model

`spec/system.yaml` and `spec/domains/operations.yaml`, selected and pinned by `spec/ess-inputs.yaml`.
This source-derived projection covers accepted-operation rows, uncertain dispatch, terminal success/error, absent identity and wrong-state refusals. It does not cover the full service; `spec/coverage.md` names all excluded boundaries and the disconnected-creation-state compiler finding.

## Validation

`ess specify validate --path spec` reports `substrate v1 — 2 file(s), valid`.
`ess verify conform synthesize --path spec --out spec/operations-suite.json` reports 14 scenarios, 0 authored, 0 refusals.
The committed suite runs in `crates/substrate-store/tests/ess_operations.rs` against the production SQLite Store, observing persisted rows and event observations. The native gate rejects changed suite bytes and unsupported steps.

## Negative control

Temporarily retaining the Accepted state in the production mark_dispatch_unknown SQL makes the Unknown-state refusal scenario fail: observed outcome unknown, expected not-accepted. The original SQL was restored. No runtime change is part of this adoption.
