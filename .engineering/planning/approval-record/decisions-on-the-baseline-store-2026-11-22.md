---
format: aep.planning-md/3
id: approval-record:decisions-on-the-baseline-store-2026-11-22
kind: approval-record
status: draft
title: 'Decisions on the baseline store: D18 on default, D19 by the operator (2026-11-22)'
relations:
- decides: story:daemon-holds-the-baseline-objects-of-a-git-workspace
revision: 1
---
# Decisions on the baseline store: D18 on default, D19 by the operator (2026-11-22)

## D18: does the store count against the workspace's storage limit?

Put to the operator through session `host-operator` at 15:11 CET as its decision H11, with
options A (not counted, bounded by the same byte limit; recommended, default) and B (counted
under the workspace's project quota). No answer arrived within the 15 minutes of the
collection's rule "A default proceeds". **Taken on default: A.** It is not a decision of the
operator and not one of `host-operator`. B stays possible: it changes no wire and no stored
format.

## D19: Git workspaces created before the store

Put to the operator through `host-operator` at about 19:55 CET after the second and third
review: A, keep the one-time build of a store from the workspace's own object files,
hardened; B, no build, such a workspace answers both Git observations
`workspace.git-baseline-missing` until it is created again.

**Decided by the operator: A.** His pick reached this session as a relay by `host-operator`;
it was not typed here. The words, as relayed:

> host-operator relays Timo's pick, 20:31:47 CET, typed in the host-operator session: D19
> "A: keep one-time build". Your default stands as his word.

The default had been taken at 20:12 CET; the pick confirms it.
