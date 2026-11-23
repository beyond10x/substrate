---
format: aep.planning-md/3
id: story:every-success-result-fits-the-ledger-before-its-side-effect
kind: story
status: draft
title: Every success result fits the ledger before its side effect
tags:
- security
scope:
- confidence: inferred
  path: crates/substrate-daemon/src/app/operations.rs
- confidence: inferred
  path: crates/substrate-wire/src/lib.rs
revision: 2
---
# Story: every success result fits the ledger before its side effect

## Outcome

Every mutation whose success result the operation ledger stores is decided against the bytes
the ledger reserves for a success result (`StoreConfig::success_result_json_bytes`) before the
mutation's side effect, as the v2 file changes are since release `0.25.0`.

## The rule

For each operation kind the daemon either proves from the wire types that the widest result
fits the reserve, or refuses before dispatch a request for which no result can fit, and holds
the driver's answer to the reserve at completion.

## What changes

The kinds not yet decided this way: an exec start, an exec signal and a host-process start.
For each, the wire crate states the widest JSON form of its result, and a test holds that
number under the default reserve. Where a caller's input sets the width (an argument list,
an environment echoed in the applied record), the daemon refuses before dispatch.

## Design

No wire schema, route or refusal code changes. The refusal is a published one of the
operation.

## Acceptance

One case per kind: a request built to give the widest admitted result completes, its row is
terminal, and the same operation id replays the stored result. One case per kind that a
caller's input can widen: the request is refused before dispatch and nothing started.
