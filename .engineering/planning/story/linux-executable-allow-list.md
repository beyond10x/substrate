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
- confidence: inferred
  path: crates/substrate-exec-gate
- confidence: cited
  path: crates/substrate-host/src/confinement/linux.rs
- confidence: inferred
  path: crates/substrate-host/src/lib.rs
- confidence: cited
  path: crates/substrate-host/src/probe.rs
- confidence: inferred
  path: crates/substrate-host/src/process.rs
- confidence: inferred
  path: crates/substrate-host/tests/confinement-public-items.txt
- confidence: inferred
  path: deploy/helm/substrate
- confidence: inferred
  path: docs/design/35-executable-lists.md
- confidence: inferred
  path: scripts/delegated-lane.sh
- confidence: inferred
  path: spec/confinement-suite.json
- confidence: cited
  path: spec/confinement/confinement.yaml
- confidence: inferred
  path: spec/confinement/scenarios
- confidence: inferred
  path: website/docs
revision: 14
transitions:
- {from: "draft", to: "proposed", at: "2026-11-21T02:33:42Z", actor: "human:timo", revision: 7, decided_on: {"recorded":{"review_outcome":1}}, correlation: "run-governance-first-slice-accepted"}
- {from: "proposed", to: "active", at: "2026-11-21T02:33:42Z", actor: "human:timo", revision: 8, decided_on: {"recorded":{"review_outcome":1}}, correlation: "run-governance-first-slice-accepted"}
---
# Story: The `linux` adapter holds a run to an executable allow-list

## Outcome

A profile opened on the `linux` adapter with a non-empty `exec_subpaths` is held to it: `execve`
of a path that is neither the literal program nor beneath a listed path fails inside the run. The
adapter proves the fact `exec_subpaths` only where its probe saw such an execution denied.

The list restricts which files `execve` accepts. It does not restrict what code an allowed program
runs: an allowed interpreter runs any script it can read, the dynamic loader started directly runs
any program file the run can read, and a memfd copy of a program runs. The draft of
`docs/design/35-executable-lists.md` records each as a probe result of 2026-11-21. The list holds
a run that starts programs by path; it is not containment of a program that sets out to run other
code.

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
   Probes of 2026-11-21 on this workstation (kernel 6.18, Landlock ABI 7, bubblewrap 0.13.0),
   recorded in the draft document, already rule two candidates out: Landlock applied before
   bubblewrap makes bubblewrap's first mount fail, and no bubblewrap version has a `noexec`
   option. The draft's mechanism is Landlock's execute right applied inside the sandbox by a
   helper that runs last before the program; it needs Landlock ABI 2 (Linux 5.19).
2. `spec/confinement/confinement.yaml` declares `exec_subpaths` for `linux`, with a scenario for
   the admission of a profile that sets the list. The confinement model has only `OpenAdapter`
   and `AdmitProfile`, so the allowed and the denied execution are delegated-lane cases, not
   scenarios. `cargo xtask check-ess` regenerates the suite.
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

- The design document is accepted before any code of this story
  (`approval-record:sdc-accepts-design-35-2026-11-21`, at commit `ab22126ff`), and the story's
  branch is stacked on the design branch, so its code reaches `main` only after the document.
- On a host where the probe cannot prove the mechanism, the fact is absent and a profile that sets
  `exec_subpaths` is refused `confinement.fact-unserved`; nothing runs unrestricted (invariant 3).
- A profile with an empty `exec_subpaths` renders and runs as before this story.
- `the_published_set_is_the_linux_adapters_proof` passes with `exec_subpaths` proved by the
  adapter and absent from the published set, and the runtime vectors' delegated lane still lists
  `whole-tree-kill`, `ipc-socket-allowlist` and `ipc-signals-contained` (`AGENTS.md` § The gate).
- An entry's class and the helper's path are decided by the place of a path (design 35, D25).
  Where the place cannot be established the profile or the open is refused by name; no code path
  compares path text instead. A profile with an empty list takes no place check.
- With a helper configured and a path the open checks on a file system outside the closed list
  (`ext4`, `overlay`, `tmpfs`), the open succeeds, the adapter proves neither list fact, and a
  profile with a list is refused `confinement.fact-unserved`; nothing runs with a list that is
  not enforced. The open's result, the adapter's capability report and that refusal each name the
  cause, the file-system type and the path
  (`approval-record:sdc-accepts-d26-and-withheld-facts-2026-11-21`; the design's fifth revision
  names the items).
- The chart's documentation says that the executable lists are served only on ext4, overlay and
  tmpfs volumes today.
- With `/usr` listed, `/bin/id` is not executable; with `/usr/bin` listed it is. A lane case
  holds that limit.
- A delegated-lane case holds each stated limit as it is: with `/usr/bin` listed, `sh script` runs
  a script outside the list, and a `#!` script outside the list is refused.
- A run with a list can still rename a file within one directory and across two unlisted
  directories.
- `cargo xtask check-ess` and `bash scripts/delegated-lane.sh` pass, and the lane builds and
  names the helper.

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
- The helper's source (a new binary of this repository), `crates/substrate-host/src/lib.rs`
  (`HostConfig`), `scripts/delegated-lane.sh`. Inferred from the draft design document.
- `crates/substrate-host/tests/confinement-public-items.txt`. Inferred: the public `linux::proof`
  gains an argument in the draft design.
- `website/docs`. Inferred.
- `deploy/helm/substrate`. Inferred: the chart's sentence about the served file systems.
