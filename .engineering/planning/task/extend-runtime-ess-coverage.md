---
format: aep.planning-md/3
id: task:extend-runtime-ess-coverage
kind: task
status: draft
title: Extend ESS coverage across the shipped Substrate runtime
relations:
- depends_on: story:standalone-docs-and-ess-adoption
revision: 1
---
## Outcome

Extend the initial accepted-operation ledger specification to the remaining shipped runtime boundaries. This is an explicit coverage backlog, not a claim that the first projection specifies all of Substrate.

## Source inventory

- Operation refusal before acceptance, idempotent replay/conflict, capacity and restart recovery: `crates/substrate-store/src/operations.rs`, `recovery.rs` and native store tests.
- Workspace admission, mutation, destruction and cleanup: `crates/substrate-store/src/workspaces.rs` and `crates/substrate-daemon/src/app/workspaces.rs`.
- Exec and session lifecycle, attachment and retirement: store `execs.rs`, `sessions.rs`; daemon `app/execs.rs`, `app/sessions.rs`.
- Leases, snapshots and events: store `leases.rs`, `snapshots.rs`, `events.rs`.
- Filesystem confinement, resource bounds, apertures and sealed descriptors: `crates/substrate-host/src/` and the portable/delegated runtime vectors.
- Transport and hosted/delegated authority: daemon `hosted.rs`, `delegation.rs` and the immutable 0.16.0 wire contract.

## Acceptance

Derive each domain from the existing implementation and contract without inventing lifecycle transitions. Run every synthesized scenario against the production boundary, fail on unsupported steps, record missing fixture lanes explicitly, and show a named failing scenario after deliberately breaking each relied-on mapping. Keep the original bundle and native runtime gates. A disconnected refused-operation creation state must be resolved in ESS or represented by an explicitly reviewed source-derived projection before it is claimed covered.

## Scope decision

The operator has been asked whether to include this whole boundary in the first adoption or deliver the ledger first. This draft preserves the remaining work while that answer is pending; it does not record approval or schedule parallel implementation.
