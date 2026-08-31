---
format: aep.planning-md/1
id: epic:container-driver-entry
kind: epic
status: proposed
title: Container driver entry
summary: 'Plan 03''s entry criteria proven before any Docker code: no host type crosses the driver port, one driver-parameterised conformance journey, named root-equivalent Docker authorities.'
owner: substrate
tags:
- phase-5
relations:
- depends_on: epic:byte-plane-completion
revision: 5
---
# Epic: Container driver entry

## Outcome

Plan 03's entry criteria are proven mechanically before any Docker code exists, so a second driver
tests the contract instead of the contract bending around it. Plan 03 moves from *deferred* to
*active* and `ROADMAP.md` phase 5 to *in progress* in the same change.

## Why Now

`docs/plan/03-container-driver.md` is *deferred until host conformance* and names four entry
criteria. The first — the minimum host slice passes public-wire and driver conformance — is met
(`STATUS.md`, phase 3 complete). The other three are claims nobody has checked: driver ports
contain no host library types; capability predicates and applied-enforcement observations are
stable; the security design states which Docker authorities are root-equivalent.

`ROADMAP.md` forbids starting phase 5 while a phase-4 exit criterion is open, so this epic holds
one story and depends on `epic:byte-plane-completion`. It exists now so that the entry gate is
written down before anyone writes a `docker` module.

## Scope

`story:docker-driver-entry-gate`: a structural test that no `substrate-host` type crosses the
driver port; the workspace/exec conformance journey as a driver-parameterised harness the host
driver passes unchanged; the design 04 section naming root-equivalent Docker authorities.

## Out of Scope

The Docker driver itself, image pull/inspection, workload lifecycle — plan 03 § *Planned proof*,
which starts once this epic is implemented.

## Risks

The structural test's shape: a compile-time trait-bound check, a `cargo` dependency-graph
assertion, or a source scan. Prefer the one that fails on the first offending `use`.

## Done When

The story is implemented, and plan 03's status and `ROADMAP.md` phase 5 change together.

## Correction — 2026-08-30

The body above says this epic holds one story and is done when that story is implemented. The
2026-08-29 split gave it **two**: `story:driver-port-carries-no-host-types`, which is already
implemented, and `story:docker-driver-entry-gate`, which is a draft now carrying a proposed design
(`docs/design/15-docker-driver-entry-gate.md`).

"Done when the story is implemented" therefore reads as satisfied when it is not. This epic is done
when **both** are, and the Docker gate is the one that remains — a gate stated before any Docker
code exists, whose design says sealed secret slots and egress apertures are **absent** on a
container driver rather than weakened.

## Reconciled delivery — 2026-09-01

The structural driver-port story is implemented. The proposed Docker entry gate remains first, followed by a workspace/exec slice and then an immutable image-backed workload slice. Both implementation slices depend on shared black-box conformance and retain absent facts for secret slots and apertures until a separate mechanism proves them.
