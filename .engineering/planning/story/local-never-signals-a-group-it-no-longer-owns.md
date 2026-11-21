---
format: aep.planning-md/3
id: story:local-never-signals-a-group-it-no-longer-owns
kind: story
status: draft
title: local never signals a process group it no longer owns
tags:
- defect
- loom
scope:
- confidence: cited
  path: crates/substrate-host/src/confinement/live.rs
- confidence: cited
  path: crates/substrate-host/src/confinement/local.rs
- confidence: inferred
  path: crates/substrate-host/src/confinement/systemd.rs
- confidence: inferred
  path: crates/substrate-host/tests
revision: 3
---
# Story: `local` never signals a process group it no longer owns

## Outcome

A `signal` on a `local` run whose leader has been reaped by anything other than the run's own
task sends nothing. Today it can send the signal to an unrelated process group that reused the
leader's pid.

## Where the requirement comes from

Session `loom` reported it on 2026-11-21 from a probe of `local` at 0.21.0: read from source and
seen in one run, not held by a failing test. Loom cannot reach it itself (its `RunId` lives inside
one `block_on`). Stakeholder: session `loom` as the reporter; the defect is Substrate's own.

## Why

- A `local` run's leader is its process group id. `signal` sends `kill(-pid, sig)` unless the
  run's `reaped` flag is set (`crates/substrate-host/src/confinement/local.rs:436-442`).
- Only the run's task sets that flag, directly before it reaps the leader
  (`crates/substrate-host/src/confinement/local.rs:313-320`). The task owns the
  `tokio::process::Child`.
- When the caller drops the tokio runtime while the task still waits (the leader has exited, and
  a descendant outside the group holds its output), the task is cancelled and the `Child` is
  dropped. tokio's drop calls `try_wait`, which reaps the zombie leader (tokio 1.53.2,
  `src/process/unix/reap.rs:122-126`, read in the cargo registry; `Cargo.lock` pins 1.53.1, whose
  file was not read). The flag stays `false`.
- The adapter still holds the run. A later `signal` on that `RunId` passes the guard and signals
  whatever process group has the id by then. In Loom's run an unrelated group had taken the
  leader's pid.
- The live-output kill hook has the same guard (`crates/substrate-host/src/confinement/live.rs:70-81`).
- `AGENTS.md` § *Safety envelope* makes whole-tree kill part of the floor for confined runs.
  `local` is declared insecure and does not prove whole-tree kill, but a signal that reaches a
  process the run never started is outside anything the adapter states.

## What changes

1. A failing test first: a run whose leader exits while a descendant outside the group holds its
   output, on a runtime that is then dropped; a `signal` on the kept adapter must not call
   `kill`. The test observes the call, it does not wait for pid reuse.
2. The fix is the implementor's to choose and state. Candidates, not decided: the flag is set by
   a guard that runs when the task is dropped; or the run keeps a pidfd of the leader and signals
   only while it shows the leader unreaped; or `signal` checks that the pid is still this run's
   zombie child before it signals the group.
3. The same rule for the live-output kill hook.
4. The module documentation of `local.rs` states what a dropped runtime does to a run.

## Acceptance

After the runtime that started a `local` run is dropped, `signal` on that run returns without
sending a signal, and `wait` on it answers instead of hanging; before this story the same `signal`
calls `kill` on the leader's former process group id.

Checks:

- The failing test of step 1 is red before the fix and green after it.
- A run on a live runtime is signalled as today: the existing `local` cases pass unchanged.
- The `systemd` adapter around `local` is checked for the same path, and the result is stated.

## Boundaries

- No change to which processes a `Kill` reaches on `local`: a descendant outside the group is
  still not signalled (answered to session `loom` on 2026-11-21, no change asked for).
- No contract or wire change.

## Order

Independent of `epic:runs-are-governed-and-limit-hits-are-events`. It shares
`crates/substrate-host/src/confinement/local.rs` with no active story of that epic.

## Scope

- `crates/substrate-host/src/confinement/local.rs`. Cited: `:313-320`, `:436-442`.
- `crates/substrate-host/src/confinement/live.rs`. Cited: `:70-81`.
- `crates/substrate-host/src/confinement/systemd.rs`. Inferred.
- `crates/substrate-host/tests`. Inferred.
