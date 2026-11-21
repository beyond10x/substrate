---
format: aep.planning-md/3
id: story:linux-executable-allow-list
kind: story
status: active
title: The linux adapter holds a run to an executable allow-list
summary: 'The linux adapter proves exec_subpaths: a run executes only its program and programs beneath the listed paths. Design document first.'
owner: substrate
tags:
- confinement
- sdc
relations:
- decomposes: epic:runs-are-governed-and-limit-hits-are-events
scope:
- confidence: cited
  path: crates/substrate-host/src/confinement/linux.rs
- confidence: cited
  path: crates/substrate-host/src/probe.rs
- confidence: inferred
  path: crates/substrate-host/src/process.rs
- confidence: inferred
  path: docs/design/35-executable-lists.md
- confidence: inferred
  path: spec/confinement-suite.json
- confidence: cited
  path: spec/confinement/confinement.yaml
- confidence: inferred
  path: spec/confinement/scenarios
- confidence: inferred
  path: website/docs
revision: 8
transitions:
- {from: "draft", to: "proposed", at: "2026-11-21T02:33:42Z", actor: "human:timo", revision: 7, decided_on: {"recorded":{"review_outcome":1}}, correlation: "run-governance-first-slice-accepted"}
- {from: "proposed", to: "active", at: "2026-11-21T02:33:42Z", actor: "human:timo", revision: 8, decided_on: {"recorded":{"review_outcome":1}}, correlation: "run-governance-first-slice-accepted"}
---
# Story: The `linux` adapter holds a run to an executable allow-list

## Outcome

A profile opened on the `linux` adapter with a non-empty `exec_subpaths` runs only its literal
program and programs beneath the listed paths. Executing anything else fails inside the run. The
adapter proves the fact `exec_subpaths` only where its probe saw such an execution denied.

## Why

- Only `macos` enforces the list today (`crates/substrate-host/src/confinement/macos.rs:3-5`,
  `:28-35`, `:574`).
- `linux` does not prove the fact, so a profile that sets `exec_subpaths` is refused
  `confinement.fact-unserved` (`crates/substrate-host/src/confinement/linux.rs:29-31`,
  `crates/substrate-host/src/confinement/profile.rs:216`).
- The specification leaves the fact unmapped for `linux` until a story declares it
  (`spec/confinement/confinement.yaml:199-201`).
- A confined child on `linux` can execute every program its filesystem view holds. No rule in
  `crates/substrate-host/src/confinement/linux.rs` restricts execution.

## What changes

1. **A design document first** (`AGENTS.md` invariant 8), `docs/design/35-executable-lists.md`. It
   decides:
   - the rule for `linux`: a profile with a non-empty `exec_subpaths` is held to the program and
     the list; a profile with none runs as today. On `macos` a profile with none runs only its
     program (`macos.rs:3-5`); the document states that difference or removes it;
   - whether an entry may name one file as well as a directory;
   - the mechanism. Candidates, inferred and not decided: Landlock's execute right applied to the
     run before its program starts (this workstation's kernel lists `landlock` in
     `/sys/kernel/security/lsm`; the repository holds no Landlock code), or a filesystem view whose
     other mounts are `noexec`. The mechanism must also be able to deny one program beneath an
     allowed directory, which `story:executable-deny-list` needs;
   - what an allowed interpreter means: an allowed `sh` or `python` runs any script it can read.
     The document states this limit;
   - the probe that proves the fact, and what the run sees on a denied execution;
   - whether the adapter can observe a denied execution. The document states the answer. No story
     of the epic promises a record of one;
   - `systemd` around `linux` takes `linux`'s set (`spec/confinement/confinement.yaml:211`);
     `systemd` around `local` stays without the fact.
2. `spec/confinement/confinement.yaml` declares `exec_subpaths` for `linux`, with a scenario for an
   allowed and a denied execution; `cargo xtask check-ess` regenerates the suite.
3. The `linux` adapter applies the list and publishes the fact from its probe
   (`linux::proof`, `crates/substrate-host/src/confinement/linux.rs:166`).
4. **The daemon does not publish the fact yet.** `publish_confinement`
   (`crates/substrate-host/src/probe.rs:269`) leaves `confinement.exec-subpaths` unpublished in
   this story although `linux::proof` proves it. The test
   `the_published_set_is_the_linux_adapters_proof` (`crates/substrate-host/src/probe.rs:1146-1218`)
   holds the two sets equal today; this story changes it to state that one exception by name. The
   reason: a wire start has no field for the list, so a client that required the property would be
   admitted and its run would execute anything. `story:exec-start-carries-executable-lists` adds
   the field and removes the exception.

## Acceptance

A delegated-lane run on the `linux` adapter whose profile lists one directory executes a program
beneath it, and an execution of a program outside the list fails inside that run; before this
story the same profile is refused `confinement.fact-unserved`.

Checks:

- The design document is merged before any code of this story.
- On a host where the probe cannot prove the mechanism, the fact is absent and a profile that sets
  `exec_subpaths` is refused `confinement.fact-unserved`; nothing runs unrestricted (invariant 3).
- A profile with an empty `exec_subpaths` renders and runs as before this story.
- `the_published_set_is_the_linux_adapters_proof` passes with `exec_subpaths` proved by the
  adapter and absent from the published set, and the runtime vectors' delegated lane still lists
  `whole-tree-kill`, `ipc-socket-allowlist` and `ipc-signals-contained` (`AGENTS.md` § The gate).
- `cargo xtask check-ess` and `bash scripts/delegated-lane.sh` pass.

## Boundaries

- No deny-list: `story:executable-deny-list`.
- No `oci` or `pod` change: `story:executable-lists-on-oci-and-pod`.
- No wire or bundle change: `story:exec-start-carries-executable-lists`.

## Order

First story of `epic:runs-are-governed-and-limit-hits-are-events`. It depends on nothing.

## Scope

- `docs/design/35-executable-lists.md` (new). Inferred.
- `spec/confinement/confinement.yaml`, `spec/confinement/scenarios`, `spec/confinement-suite.json`.
  Cited: `spec/confinement/confinement.yaml:199-201`.
- `crates/substrate-host/src/confinement/linux.rs`. Cited: `:29-31`, `:166`, `:200-206`.
- `crates/substrate-host/src/probe.rs`. Cited: `:269`, `:1146-1218`.
- `crates/substrate-host/src/process.rs`. Inferred: the bubblewrap argv is built there.
