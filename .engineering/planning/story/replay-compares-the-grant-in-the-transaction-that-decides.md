---
format: aep.planning-md/3
id: story:replay-compares-the-grant-in-the-transaction-that-decides
kind: story
status: draft
title: A stored outcome is returned only after the grant is compared in the transaction that decides the replay
summary: ADR 0017's grant comparison holds for concurrent retries
tags:
- defect
- security
revision: 2
---
# Story: a stored outcome is returned only after the grant is compared in the transaction that decides the replay

## Outcome

ADR 0017 holds for concurrent requests: whichever request finds a stored row, in whichever
transaction, gets the stored outcome only when its verified grant matches the row's, and
otherwise the refusal ADR 0017 names. A refusal that was decided is not replaced by another
request's outcome.

## What changes

`crates/substrate-store/src/operations.rs` and `crates/substrate-daemon/src/app/operations.rs`:
the grant is part of what the deciding transaction compares, beside the request hash.

## Design

No wire schema changes. ADR 0017 is the design; the story adds the concurrent case to it.

## Acceptance

A store-level case with two requests of one subject and one operation identity that differ in
their delegated context: the second gets the named refusal in every interleaving the case
drives; red before the change.


## Where the requirement comes from

The operator's private security review of `main` at `1af046902`, dated 2026-11-22,
finding 4. The operator ordered its findings handled on 2026-11-22. By
`approval-record:operator-decides-how-security-review-fixes-travel-2026-11-22` this story
states the rule that must hold; the review's detail stays with the operator until the fix is
on `main`. Checked against the source at `1af046902` by reading; nothing was run. Release
0.23.0 has the same source. Stakeholder: the operator.

## Delivery

## Delivery

Branch `story/replay-grant`, pull request 265. The comparison is one function in the store,
used by twelve entry points; the daemon maps the store's answer to the existing refusal. ADR
0017 carries an addendum for requests in flight together. Reviewed once
(`review-result:replay-grant-security-1`): no rule unheld in code, text narrowed to mutation
requests, eleven cases of the reviewer kept.
