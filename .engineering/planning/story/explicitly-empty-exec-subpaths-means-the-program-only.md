---
format: aep.planning-md/3
id: story:explicitly-empty-exec-subpaths-means-the-program-only
kind: story
status: draft
title: An explicitly empty executable allow-list means the program only
summary: 'exec_subpaths tells absent from empty: absent keeps today''s behaviour, empty holds the run to its program on linux and macos. Design 35 is amended first; decided when the wire story is planned.'
owner: substrate
tags:
- confinement
- sdc
relations:
- decomposes: epic:runs-are-governed-and-limit-hits-are-events
- depends_on: story:executable-deny-list
scope:
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: crates/substrate-host/src/confinement/linux.rs
- confidence: cited
  path: crates/substrate-host/src/confinement/macos.rs
- confidence: cited
  path: crates/substrate-host/src/confinement/profile.rs
- confidence: cited
  path: crates/substrate-host/tests/confinement-public-items.txt
- confidence: inferred
  path: docs/design/35-executable-lists.md
- confidence: inferred
  path: spec/confinement-suite.json
- confidence: inferred
  path: spec/confinement/confinement.yaml
- confidence: inferred
  path: spec/confinement/scenarios
revision: 3
---
# Story: An explicitly empty executable allow-list means the program only

## Outcome

A profile can say one of three things about its executable allow-list: nothing, which keeps
today's behaviour on each adapter; an empty list, which holds the run to its literal program on
every adapter that proves `exec_subpaths`; or entries. A client that wants the strict rule on
`linux` no longer has to list the program itself.

## Where the requirement comes from

Session `sdc`'s review of `docs/design/35-executable-lists.md` on 2026-11-21 added this as option
C of that document's question on the empty list. Session `sdc`, the stakeholder, decided option A
for `story:linux-executable-allow-list` and `story:executable-deny-list` and asked for C as a
draft follow-up, to be decided when `story:exec-start-carries-executable-lists` is planned
(`approval-record:sdc-decides-design-35-defaults-2026-11-21`).

## Why

- `ConfinementProfile.exec_subpaths` is a `Vec<String>`
  (`crates/substrate-host/src/confinement/profile.rs:72`). It cannot tell "not set" from "set and
  empty".
- Under design 35 as decided, an empty list on `linux` runs as today, and on `macos` it holds the
  run to its program (`crates/substrate-host/src/confinement/macos.rs:3-5`). One profile means two
  things on two adapters.
- The fact a profile requires is derived from the list being non-empty
  (`crates/substrate-host/src/confinement/profile.rs:216`), so an empty list requires no fact and
  is admitted by an adapter that cannot enforce anything.

## What changes

1. **`docs/design/35-executable-lists.md` is amended first** (`AGENTS.md` invariant 8). The
   amendment decides the field's type (an optional list), what an absent list means on `macos`
   (today it is the program only; whether that stays), that an empty list requires the fact
   `exec_subpaths`, the deny-list rule beside an empty allow-list, and consumer compatibility for
   every embedder that builds the profile with `..Default::default()`.
2. `spec/confinement/confinement.yaml` states the three cases and their admission;
   `cargo xtask check-ess` regenerates the suite.
3. `linux` and `macos` enforce the empty list as "the program only".

## Acceptance

In the delegated lane, a run on the `linux` adapter whose profile sets an explicitly empty
`exec_subpaths` executes its literal program, and an `execve` of any other path fails inside that
run; before this story the same profile runs unrestricted.

Checks:

- The amendment to design 35 is merged before any code of this story.
- A profile that does not set the list runs on `linux` as before this story.
- A profile with an explicitly empty list on `local` is refused `confinement.fact-unserved`
  naming `exec_subpaths`.
- The same strict case passes on the `macos-15` lane.
- `crates/substrate-host/tests/confinement-public-items.txt` is regenerated in the same change,
  and the change log names the field's new type as a breaking change of the in-process API.

## Boundaries

- No wire change. `story:exec-start-carries-executable-lists` decides how the wire field tells
  absent from empty.
- No change to what a non-empty list means.

## Order

After `story:executable-deny-list`: it changes the same profile field, facts and design document.
Decided, and started or dropped, when `story:exec-start-carries-executable-lists` is planned.

## Scope

- `docs/design/35-executable-lists.md`. Inferred.
- `spec/confinement/confinement.yaml`, `spec/confinement/scenarios`, `spec/confinement-suite.json`.
  Inferred.
- `crates/substrate-host/src/confinement/profile.rs`. Cited: `:72`, `:216`.
- `crates/substrate-host/src/confinement/linux.rs`, `crates/substrate-host/src/confinement/macos.rs`.
  Cited: `macos.rs:3-5`.
- `crates/substrate-host/tests/confinement-public-items.txt`. Cited: `:1-3`.
- `CHANGELOG.md`. Inferred.
