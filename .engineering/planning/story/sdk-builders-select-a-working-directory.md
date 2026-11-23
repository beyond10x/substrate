---
format: aep.planning-md/3
id: story:sdk-builders-select-a-working-directory
kind: story
status: draft
title: The Rust SDK's exec and session builders select a working directory
summary: The wire publishes workdir (design 26); both SDK builders send None
tags:
- defect
revision: 1
---
# Story: the Rust SDK's exec and session builders select a working directory

## Outcome

An application using the SDK's exec builder or session builder starts a program in a working
directory beneath the workspace or a declared root, as design 26 publishes it.

## What was seen

Read at release 0.24.0: both builders in `crates/b10x-substrate-sdk/src/lib.rs` set
`workdir: None`. The wire's start inputs carry `workdir` since bundle `0.21.0`.

## What changes

A `workdir` method on both builders, with the lexical checks the wire crate already has, and
the refusals of design 26 surfaced as the SDK surfaces other refusals. The public SDK page names
it.

## Design

No contract changes; design 26 is the design. Beside `story:sdk-aperture-list`, which publishes
the other half of design 26 in the SDK.

## Acceptance

An SDK case against the shipped daemon on the delegated lane: a program started with a working
directory beneath the workspace prints that directory. On the portable lane: the request carries
the member, and an absent `exec.workdir` fact is the named refusal. Red before the change.

## Where the requirement comes from

A review of release 0.24.0 dated 2026-11-23, finding 8, which the operator handed to this
session on 2026-11-23 with the order to address every finding. Stakeholder: the operator.
