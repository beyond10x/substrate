---
format: aep.planning-md/3
id: story:executable-deny-list
kind: story
status: active
title: A profile names executables that never run
summary: ConfinementProfile gains a deny-list and a fact for it; linux and macos enforce it, the other adapters refuse it by name.
owner: substrate
tags:
- confinement
- sdc
relations:
- decomposes: epic:runs-are-governed-and-limit-hits-are-events
- depends_on: story:linux-executable-allow-list
scope:
- confidence: cited
  path: crates/substrate-host/src/confinement/facts.rs
- confidence: inferred
  path: crates/substrate-host/src/confinement/linux.rs
- confidence: inferred
  path: crates/substrate-host/src/confinement/macos.rs
- confidence: cited
  path: crates/substrate-host/src/confinement/mod.rs
- confidence: cited
  path: crates/substrate-host/src/confinement/profile.rs
- confidence: inferred
  path: crates/substrate-host/src/probe.rs
- confidence: inferred
  path: crates/substrate-host/src/seatbelt.rs
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
- confidence: inferred
  path: spec/generated
- confidence: inferred
  path: website/docs
revision: 11
transitions:
- {from: "draft", to: "proposed", at: "2026-11-21T02:33:42Z", actor: "human:timo", revision: 10, decided_on: {"recorded":{"review_outcome":1}}, correlation: "run-governance-first-slice-accepted"}
- {from: "proposed", to: "active", at: "2026-11-21T02:33:42Z", actor: "human:timo", revision: 11, decided_on: {"recorded":{"review_outcome":1}}, correlation: "run-governance-first-slice-accepted"}
---
# Story: A profile names executables that never run

## Outcome

A `ConfinementProfile` carries a deny-list of executables beside its allow-list. A run on an
adapter that proves the new fact cannot execute a listed program, even where the allow-list admits
its directory. An adapter that does not prove the fact refuses the profile by the fact's name, and
a deny-list without an allow-list is refused by name on every adapter.

## Why

- No adapter has a deny-list. `ConfinementProfile` has no field for one
  (`crates/substrate-host/src/confinement/profile.rs:43-97`), and `FACT_ORDER` has no fact for one
  (`crates/substrate-host/src/confinement/facts.rs:28-42`).
- The relayed requirement of 2026-11-21 names "an executable allow-list and deny-list"
  (`epic:runs-are-governed-and-limit-hits-are-events`).
- A run that can write and execute a file in its workspace can copy a denied program there. A
  deny-list therefore means something only beside an allow-list that does not admit the workspace.
  Session `sdc` answered on 2026-11-21 that Loom always sends both (its own answer, reversible).

## What changes

1. **The design document of `story:linux-executable-allow-list` gains the deny-list** before code
   (`AGENTS.md` invariant 8). It decides:
   - the field's name and entry form on `ConfinementProfile`, and its bound;
   - the new fact's name. Its place in the `Fact` order is directly after `exec_subpaths`, so an
     adapter that proves neither fact refuses a profile with both lists naming `exec_subpaths`
     (admission names the first unproved fact, `crates/substrate-host/src/confinement/mod.rs:631-637`);
   - precedence: a denied entry wins over an allowed subpath;
   - the name and status of the refusal for a deny-list beside an empty `exec_subpaths`, and the
     sentence that states the copy-into-the-workspace limit;
   - whether a denied program reached through a symbolic link or a hard link is denied;
   - the mechanism on `linux` and on `macos` (Seatbelt has `deny` rules,
     `crates/substrate-host/src/confinement/macos.rs:1414-1416` shows the form for writes).
2. `spec/confinement/confinement.yaml` gains the field, the fact, the refusal and scenarios;
   `cargo xtask check-ess` regenerates the suite and the generated types.
3. `linux` and `macos` enforce the list and prove the fact from their probes. `local`, `oci`,
   `pod` and `systemd` around `local` do not prove it and refuse the profile
   `confinement.fact-unserved`.
4. `publish_confinement` leaves the new fact unpublished, as it does `exec_subpaths` after
   `story:linux-executable-allow-list`; the wire has no property for it until
   `story:exec-start-carries-executable-lists`.

## Acceptance

A delegated-lane run on the `linux` adapter whose profile allows `/usr/bin` and denies one program
beneath it executes another program in `/usr/bin`, and an execution of the denied program fails
inside that run; before this story the profile type has no deny-list field.

Checks:

- The design document names the deny-list decisions before any code of this story.
- The same case passes on the `macos-15` lane for the `macos` adapter.
- A profile with a deny-list and an empty `exec_subpaths` is refused by the refusal the design
  names, on `linux` and on `local`, and nothing runs.
- A profile with both lists on `local`, which proves neither fact, is refused
  `confinement.fact-unserved` naming `exec_subpaths`, and its audit sink receives
  `profile_refused` with that fact.
- A unit test admits a profile with both lists against adapter facts that prove `exec_subpaths`
  and not the new fact, and the refusal names the new fact.
- A profile without a deny-list is admitted, rendered and run as before this story.
- `crates/substrate-host/tests/confinement-public-items.txt` is regenerated in the same change.

## Boundaries

- No record of a denied execution attempt (the epic's § Out of scope).
- No `oci` or `pod` enforcement: `story:executable-lists-on-oci-and-pod`.
- No wire or bundle change: `story:exec-start-carries-executable-lists`.

## Order

After `story:linux-executable-allow-list`: it uses that story's mechanism and design document.

## Scope

- `docs/design/35-executable-lists.md`. Inferred.
- `spec/confinement/confinement.yaml`, `spec/confinement/scenarios`, `spec/confinement-suite.json`,
  the generated types under `spec/generated`. Inferred.
- `crates/substrate-host/src/confinement/profile.rs`. Cited: `:43-97`, `:213-229`.
- `crates/substrate-host/src/confinement/facts.rs`. Cited: `:28-42`.
- `crates/substrate-host/tests/confinement-public-items.txt`. Cited: `:1-3`.
- `crates/substrate-host/src/confinement/linux.rs`, `crates/substrate-host/src/confinement/macos.rs`,
  `crates/substrate-host/src/seatbelt.rs`, `crates/substrate-host/src/probe.rs`. Inferred.
