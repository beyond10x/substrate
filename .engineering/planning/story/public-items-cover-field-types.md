---
format: aep.planning-md/3
id: story:public-items-cover-field-types
kind: story
status: implemented
title: The public-items freeze covers the field types of re-exported types
tags:
- adr-0109
relations:
- decomposes: epic:confinement-contract-family
- depends_on: story:confinement-consumer-surface
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-27T07:24:39Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-27T07:24:39Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-27T13:17:26Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}}
---
## Outcome

`crates/substrate-host/tests/confinement_public_items.rs` freezes the public definitions of every
type reachable from a public field or signature of the `confinement` module's re-exported types,
including `SecretSlot`, `EgressAperture` and `GitSourceBinding`, so a change to one of them fails
the test until `tests/confinement-public-items.txt` is regenerated.

## Origin

`review-result:adr-0109-u9-adversary-pass-1`: the freeze covers the re-exported types and marks
private fields, but not the types their public fields use (PR 160, "Not in this change").

## Acceptance

Adding a public field to `SecretSlot`, `EgressAperture` or `GitSourceBinding` makes
`confinement_public_items` fail with that item named; regenerating with
`SUBSTRATE_BLESS_PUBLIC_ITEMS=1` makes it pass.
