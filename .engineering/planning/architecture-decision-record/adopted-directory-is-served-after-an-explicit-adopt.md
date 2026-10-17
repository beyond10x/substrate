---
format: aep.planning-md/3
id: architecture-decision-record:adopted-directory-is-served-after-an-explicit-adopt
kind: architecture-decision-record
status: accepted
title: An operator's directory is served after an explicit adopt (bundle 0.19.0)
relations:
- supersedes: architecture-decision-record:workspace-id-is-the-root-name
- decides: decision-blocker:exec-in-adopted-directory-contract-route
revision: 2
transitions:
- {from: "proposed", to: "accepted", at: "2026-10-17T05:04:33Z", actor: "human:timo", revision: 2}
---
# ADR 0035: an operator's directory is served after an explicit adopt

## Decision
Adoption is an explicit operation, `workspace.adopt` at `POST /v1/workspaces/adopt`, taking a
`root_name` from `validate_root_name`'s set and `labels`. A successful adopt writes one store row,
Ready and owned by the calling subject, with a minted `ws_…` id, the given `root_name` and
`origin: adopted`. The wire `workspace-id` stays `^ws_[A-Za-z0-9]+$`; every route keeps addressing a
workspace by that id. Destroying an adopted workspace forgets the row (and its lease and cleanup
entry) on every removal path and leaves the directory and its contents in place. Every adopt
refusal, the out-of-set name included, is recorded in the operation ledger like every other
pre-dispatch refusal, and a retry with the same operation id replays it. The route ships in
successor bundle `0.19.0`, re-cut for this decision (invariant 6); ADR 0034's widening of
`workspace-id` is withdrawn.

## Supersedes
ADR 0034 (`architecture-decision-record:workspace-id-is-the-root-name`). Its premise, that an
adopted directory can already be read and written over the socket, does not hold: every workspace
route passes the store's `admit_workspace`, which needs a row only `workspace.create` wrote.

## Acceptance
Chosen by the operator on 2026-10-17 as route A for an exec in an adopted directory: an explicit
`workspace.adopt` route; the row is owned by the caller and a `ws_` id is minted; `root_name` is the
directory; destroy forgets the row and leaves the directory; `workspace-id` stays `ws_`; `0.19.0` is
re-cut; ADR 0034 is superseded; name refusals are recorded like every other pre-dispatch refusal.
The full record is `adr/0035-an-operators-directory-is-served-after-an-explicit-adopt.md`.
