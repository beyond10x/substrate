---
format: aep.planning-md/3
id: story:sdk-git-create-recovers-a-lost-reply
kind: story
status: draft
title: The Rust SDK recovers a Git workspace create whose reply was lost
summary: The Git create path sends once and returns the transport error without the operation id it generated
tags:
- defect
revision: 1
---
# Story: the Rust SDK recovers a Git workspace create whose reply was lost

## Outcome

A Git workspace create through the SDK whose reply is lost is recovered from the operation
ledger like every other create, and an error that leaves the outcome open carries the operation
id the SDK generated.

## What was seen

Read at release 0.24.0, not run: the ordinary create path of
`crates/b10x-substrate-sdk/src/lib.rs` asks the ledger after a transport failure
(`recover_mutation`). The Git path generates an id, sends once and returns the transport error
without the id. Design 18 states the recovery for mutations without an exception.

## What changes

The Git create goes through the same recovery as the ordinary create.

## Design

No contract changes; design 18 is the design.

## Acceptance

An SDK case with a transport that drops the reply of a Git create after the daemon accepted it:
the call returns the created workspace. Red before the change.

## Where the requirement comes from

A review of release 0.24.0 dated 2026-11-23, finding 6, which the operator handed to this
session on 2026-11-23 with the order to address every finding. Stakeholder: the operator.
