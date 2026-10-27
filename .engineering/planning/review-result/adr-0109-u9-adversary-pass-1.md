---
format: aep.planning-md/3
id: review-result:adr-0109-u9-adversary-pass-1
kind: review-result
status: active
title: 'Adversary pass 1: confinement consumer surface'
relations:
- reviews: story:confinement-consumer-surface
revision: 1
---
# Adversary pass 1: confinement consumer surface

Reviews `story:confinement-consumer-surface` at `903ba14` (merged as `3df79dbd`). Cases executed:
4 before, 15 after; 3 red.

## Findings

| # | file:line | severity | verdict | finding |
|---|---|---|---|---|
| F1 | `crates/substrate-host/src/confinement/mod.rs:55` | blocker | NEEDS-CHANGE | The public-items list records nothing about the types the module re-exports (`HostConfig`, `HostConfig::minimum`, `DeclaredWritableRoot`, `ReadOnlyRoot`, `WritableHostRoot`, `ReadOnlyFile`, `SecretSlotRequest`, `Signal`). Red: `adversary_the_list_records_the_shape_of_every_type_the_module_reexports`. |
| F2 | `crates/substrate-host/tests/confinement_public_items.rs:410` | warning | CONFIRMED | Adding a private field to an all-pub struct leaves the list unchanged. That edit breaks every consumer struct literal. Red: `adversary_a_private_field_added_to_an_all_public_struct_changes_the_list`. |
| F3 | `website/docs/guides/confinement-profiles.md:113` | warning | CONFIRMED | The page says the refusal is the first family `required_families()` names. `decide` also requires `network_egress` for `Isolated` on a `host_network` adapter, and `audit` for a sink. Red: `adversary_the_refusal_is_the_first_missing_family_the_public_helpers_name`. |

Held, by edits the scanner caught:
- trait signature, dropped bounds, new variant, moved item, retargeted `pub use`, removed `#[non_exhaustive]`, return type on its own line, lost default body.

Held in `consumer_surface`:
- `consumer_surface` chooses adapters from facts only; misreported facts turn it red.
- The delegated lane prints ABSENT, and `delegated-lane.sh` exits 1 without its count line.
- The page agrees with the code on fact, family and refusal names.

## Decisions taken by the coordinator

- F1: keep the re-exports and extend the scanner to their definitions, so the list freezes them.
- F2: record whether a struct has private fields.
- F3: correct the page to the rule `decide` applies; add no public helper.
- Notes: the module doc stops calling the adapters stubs; the page names `confinement.workspace-required`.
