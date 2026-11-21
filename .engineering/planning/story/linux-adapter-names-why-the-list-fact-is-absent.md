---
format: aep.planning-md/3
id: story:linux-adapter-names-why-the-list-fact-is-absent
kind: story
status: draft
title: The linux adapter names why the list fact is absent when a base directory shows a workspace
summary: A proof without exec_subpaths for this cause carries a withheld entry; known limit of 0.23.0
tags:
- defect
- sdc
revision: 1
---
# Story: the `linux` adapter names why the list fact is absent when a base directory shows a workspace

## Outcome

When the `linux` adapter opens without the fact `exec_subpaths` because a base directory of the
view (`/usr`, `/bin`, `/lib`, `/lib64`) shows a workspace, its proof's `withheld` names the fact
and the cause. Today the proof carries no entry for it.

## Where the requirement comes from

The third adversary round of the mount fix (pull request 244, merged as `731d06685` on
2026-11-21) reported it as note N1 and marked it as present before that change. The operator
session decided on 2026-11-21 to release 0.23.0 with the note open, as a known limit in the
changelog entry, and to file this draft before the tag. Stakeholder: session `sdc`, for whose
requirement the executable lists were built.

## Why

- Design 35 D27 says a withheld fact is in the proof with its cause
  (`AdapterFacts.withheld`, `FactUnserved.withheld`, `FactWithheld`; the proof field is in
  `crates/substrate-host/src/confinement/linux.rs`).
- In this case the adapter opens, the fact is absent and `withheld` is empty. An operator reading
  the proof cannot tell this host from one whose kernel lacks Landlock.
- It fails closed: every start with a list is refused `confinement.fact-unserved` before
  dispatch. The cost is the missing reason, in the proof and in the refusal's `withheld`.

## What changes

1. A failing test first: an open on a host staged so that a base directory shows a workspace; the
   proof must hold one `withheld` entry naming `exec_subpaths` and a cause.
2. The cause is a new member of the closed cause list or an existing one; the implementor states
   which, and design 35 § D26 or § D27 is amended before code where the list grows (invariant 8).
3. The refusal of a start with a list carries the same entry.

## Not known

- The site that drops the cause was not read for this draft. The adversary's report names the
  outcome, staged in its round-3 cases.
- Whether `systemd` around `linux` shows the same.

## Acceptance

On a host where a base directory shows a workspace, `open` returns a proof whose `withheld` names
`exec_subpaths` with a cause, and a start with a list is refused `confinement.fact-unserved`
carrying that entry; the changelog's known limit for 0.23.0 is removed in the release that ships
this.
