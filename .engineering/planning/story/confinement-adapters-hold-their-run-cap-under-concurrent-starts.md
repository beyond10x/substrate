---
format: aep.planning-md/3
id: story:confinement-adapters-hold-their-run-cap-under-concurrent-starts
kind: story
status: draft
title: A confinement adapter holds its run cap under concurrent starts
summary: Pending and running starts count against one cap, reserved before any awaited call
tags:
- defect
revision: 1
---
# Story: a confinement adapter holds its run cap under concurrent starts

## Outcome

An adapter that states a bound on its runs never holds more: a start reserves its place before
the first awaited call and releases it after cleanup, so pending and running starts share the
cap.

## What changes

`crates/substrate-host/src/confinement/pod.rs` and the `oci` adapter reserve atomically. The
`linux`, `macos` and `systemd` adapters are read for the same rule and changed where a start can
pass the check twice.

## Design

No public item changes. The refusal for a full adapter is the existing one.

## Acceptance

A case per changed adapter on its fake: more concurrent starts than the cap, with the API calls
delayed, end with the cap held and the rest refused by name; red before the change.


## Where the requirement comes from

The operator's private security review of `main` at `1af046902`, dated 2026-11-22,
finding 9. The operator ordered its findings handled on 2026-11-22. By
`approval-record:operator-decides-how-security-review-fixes-travel-2026-11-22` this story
states the rule that must hold; the review's detail stays with the operator until the fix is
on `main`. Checked against the source at `1af046902` by reading; nothing was run. Release
0.23.0 has the same source. Stakeholder: the operator.
