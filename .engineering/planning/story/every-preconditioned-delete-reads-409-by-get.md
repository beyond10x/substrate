---
format: aep.planning-md/3
id: story:every-preconditioned-delete-reads-409-by-get
kind: story
status: draft
title: Every preconditioned delete reads a 409 by GETting the name
summary: namespace.rs and exec.rs deliver read a 409 on a uid-preconditioned delete as a collision; GET the name instead
relations:
- decomposes: epic:kubernetes-deployment-and-driver
- depends_on: story:preconditioned-delete-409-is-not-a-uid-mismatch
scope:
- confidence: cited
  path: crates/substrate-kubernetes/src/exec.rs
- confidence: cited
  path: crates/substrate-kubernetes/src/namespace.rs
revision: 3
---
# Story: Every preconditioned delete reads a 409 by GETting the name

## Outcome

The namespace driver's other uid-preconditioned deletes stop treating a 409 as a collision or a
replacement. Like the Pod removal after `story:preconditioned-delete-409-is-not-a-uid-mismatch`,
each GETs the name and decides from what it reads: another uid or absent is a replacement or gone,
the same uid is retried within the call's bound.

## Finding

The u3 implementor of wave 2026-10-30-remote-conformance found two more callers that read a 409 on
`delete_preconditioned` as a uid mismatch (pre-existing, outside that story's scope):

- `crates/substrate-kubernetes/src/namespace.rs` near line 481;
- `crates/substrate-kubernetes/src/exec.rs` near line 1006 (`deliver`).

## Acceptance

- A red case first for each caller, on the fake API: a 409 whose follow-up GET still shows the
  driver's own uid; today the caller reports a collision or replacement.
- After the change, each caller GETs the name on a 409 and retries the same uid within its bound;
  the bound ends in a named outcome, never success.
- The existing collision and replacement cases stay green.

## Design

Design 28 and the uid rule of design 34 § 4. No contract change.
