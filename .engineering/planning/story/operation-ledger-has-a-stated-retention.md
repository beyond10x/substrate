---
format: aep.planning-md/3
id: story:operation-ledger-has-a-stated-retention
kind: story
status: draft
title: The operation ledger has a stated retention
summary: Whether and when terminal operation rows are removed is written down and held
tags:
- defect
revision: 1
---
# Story: the operation ledger has a stated retention

## Outcome

A design document states how long a terminal operation row and its result are kept and what
removes them, and a case holds it. The verification of the review found no statement in the
store that removes rows; it did not establish that none is intended.

## What changes

1. Read first: the store's schema and the designs of the ledger. When a retention exists, this
   story is closed with the place that states it.
2. Otherwise a design note decides the retention and its bound, before code.

## Acceptance

The retention is stated in one design document, and a store case holds it.

## Where the requirement comes from

The operator's private security review of `main` at `1af046902`, dated 2026-11-22:
a verification note beside finding 5. The operator ordered the review handled on 2026-11-22. Checked by reading;
nothing was built. Stakeholder: the operator.
