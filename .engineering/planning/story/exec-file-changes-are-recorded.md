---
format: aep.planning-md/3
id: story:exec-file-changes-are-recorded
kind: story
status: draft
title: An exec's effective file changes are recorded and attributed to that exec
summary: 'Per exec: path, op, before and after digests and diffable content, exact for concurrent execs, on the wire as a route and an event and on the in-process path. ADR and model first.'
owner: substrate
tags:
- observation
- sdc
relations:
- informed_by: epic:runs-are-governed-and-limit-hits-are-events
- depends_on: story:process-count-limit-hit-is-observed
- depends_on: story:run-observation-carries-usage
scope:
- confidence: inferred
  path: AGENTS.md
- confidence: inferred
  path: CHANGELOG.md
- confidence: inferred
  path: README.md
- confidence: inferred
  path: adr
- confidence: inferred
  path: contracts/substrate-wire
- confidence: inferred
  path: crates/substrate-daemon/src/app
- confidence: inferred
  path: crates/substrate-daemon/tests/runtime_vectors.rs
- confidence: cited
  path: crates/substrate-host/src/confinement/mod.rs
- confidence: inferred
  path: crates/substrate-host/src/probe.rs
- confidence: inferred
  path: crates/substrate-host/src/process.rs
- confidence: inferred
  path: crates/substrate-store/src
- confidence: cited
  path: crates/substrate-wire/src/lib.rs
- confidence: inferred
  path: scripts/gate.sh
- confidence: inferred
  path: spec
- confidence: inferred
  path: website/docs
- confidence: inferred
  path: xtask/bundle-source
- confidence: inferred
  path: xtask/src/bundle.rs
revision: 4
---
# Story: An exec's effective file changes are recorded and attributed to that exec

## Outcome

For every exec, Substrate records the files it changed in its writable mounts: path, operation
(create, modify, delete, rename), the content digest before and after, and content a client can
diff. The record is attributed to the exec's id whoever wrote the file inside it (an agent's tool,
a script, a formatter, a linter), and the attribution is exact when several execs run at the same
time. A client reads the record on the wire and receives an event for it; an embedder reads it on
the in-process path.

## Where the requirement comes from

Relayed by session `sdc` on 2026-11-21 as Timo's requirement of the same day. The SDC prototype
records changes itself today, with inotify and attribution by time window
(`sdc/docs/fs-changes.md` in the `sdc` repository, record `sdc.fs-change/1`). That recorder
retires once Substrate has the record.

Session `sdc` added on 2026-11-21, as its own answer and reversible: SDC runs one exec at a time
in a workspace, and Loom's planned concurrency keeps one writer per workspace, so concurrent execs
land in different workspaces. For SDC the tree-digest candidate below is enough, and an exec that
overlapped another writer in its workspace should get no record instead of a wrong one.

## Why

Verified at `main` `adfde4020`:

- Substrate records a file change only when its own file routes made it: the events
  `workspace.file-written` and `workspace.file-deleted`
  (`crates/substrate-store/src/operations.rs:854-855`). A write by a process inside an exec
  produces no record.
- `GitChangeSet` compares a Git workspace with its materialization commit
  (`crates/substrate-wire/src/lib.rs:1502-1515`, `Workspace::git_changes` at
  `crates/b10x-substrate-sdk/src/lib.rs:1108`). It is per workspace, not per exec, and exists only
  for a Git workspace.
- The exec event transitions carry no file change (`crates/substrate-wire/src/lib.rs:2308-2318`).
- `RunObservation` and `AuditRecord` carry none
  (`crates/substrate-host/src/confinement/mod.rs:280-324`, `:696-706`).
- Attribution by time window cannot separate two processes that write in the same window. Only
  the layer that starts the process and builds its filesystem view can attribute exactly.

## What changes

1. **An ADR and the model first** (`AGENTS.md` invariant 8; a new noun is modelled before its
   stories). The ADR decides:
   - the mechanism. Three candidates, each judged here from reading and not from a probe:
     - *An overlay upper directory per exec over its writable mounts.* Each exec writes into its
       own upper layer, so attribution is exact for concurrent execs without any privilege. It
       changes what an exec sees: two concurrent execs no longer see each other's writes, and the
       upper layer must be applied to the mount when the exec ends, with a rule for two execs that
       changed one path. Inferred: bubblewrap's overlay options and unprivileged overlayfs in a
       user namespace are available on the hosts the probe admits.
     - *fanotify with the writer's pid.* It keeps today's shared view. Inferred: the marks that
       report another process's pid need `CAP_SYS_ADMIN`, which the daemon does not hold
       (`README.md` § Running the daemon), so the fact would be absent on the default deployment.
     - *Tree digests of the writable mounts before and after the exec.* The simplest. It is exact
       only while one exec at a time may write the mount; with two concurrent writers it cannot
       attribute.
   - whether the record is exact by construction or the fact is absent. A host that cannot
     attribute exactly publishes no fact and records nothing under this name (invariant 3); it
     never falls back to a time window;
   - the record: its fields, how a rename and an atomic replace are reported, the bound on paths
     per exec and on content bytes kept, what is recorded for a file over the bound or a binary
     file, and how long content is kept;
   - which mounts are covered: `/workspace`, declared writable roots (ADR 0038), `/scratch`;
   - the cost: what the record adds to an exec's start and end, measured on a workstation.
2. The ESS model gains the record as an entity related to the exec, in `spec/` (a new domain or
   `spec/confinement`), validated with `ess specify validate` and held by `cargo xtask check-ess`.
3. The in-process path: the adapter exposes a run's change record, and the audit sink receives a
   record when it is complete.
4. The wire: a successor contract bundle at the next free version adds the capability fact, the
   route that reads an exec's change record and its content, the event, and each named refusal
   (invariant 6).

## Acceptance

In the delegated lane, two execs that run at the same time in two workspaces each create, modify,
delete and rename files, and each exec's change record lists exactly its own changes with the
before and after digests; before this story an exec has no change record.

Checks:

- Two execs that write one workspace at the same time: each has either a record of exactly its
  own changes or no record with the named reason the ADR gives. Neither record ever lists the
  other exec's change.
- The ADR is accepted and indexed in `adr/README.md`, and the model validates, before any code.
- A file an exec wrote and restored to its starting content yields no record.
- A change made through a tool, a shell script and a formatter inside one exec all land in that
  exec's record.
- Content for both sides of a modified text file is readable under the declared bound, and a file
  over the bound is recorded with digests and marked as over the bound.
- Portable lane: the fact is absent, the route answers its named refusal, and no exec carries a
  record.
- The successor bundle's check is in `scripts/gate.sh`; every earlier bundle is byte-unchanged.

## Boundaries

- No diff rendering, line counts or syntax highlighting. Substrate keeps digests and content; the
  client computes a diff.
- No attribution below the exec: which tool call inside an exec wrote a file stays the
  consumer's.
- No record of reads.
- Changes made through Substrate's own file routes keep their existing events.

## Open questions for the operator

- Whether this moves O1 (governed reach, `AGENTS.md` § Serves). It is an observation of what a
  confined process did inside its reach. `AGENTS.md` makes a change that moves no objective a
  question for the operator.
- Whether exact records for two execs writing one workspace at the same time are wanted. The
  relayed requirement says "exact also for concurrent processes". Tree digests give that only
  across workspaces and give no record on an overlap in one workspace; the overlay candidate
  gives it in one workspace and changes what concurrent execs see of each other's writes.

## Order

Informed by `epic:runs-are-governed-and-limit-hits-are-events` and not part of it. It cuts a
successor bundle and changes `crates/substrate-host/src/process.rs` and
`crates/substrate-host/src/confinement/mod.rs`, so it is worked after
`story:process-count-limit-hit-is-observed`, the last story of that epic's first slice.

## Scope

- `adr` (a new ADR), `adr/README.md`. Inferred.
- `spec`. Inferred.
- `crates/substrate-host/src/process.rs`, `crates/substrate-host/src/probe.rs`. Inferred: the
  filesystem view and the probe live there.
- `crates/substrate-host/src/confinement/mod.rs`. Cited: `:280-324`, `:696-706`.
- `crates/substrate-wire/src/lib.rs`. Cited: `:2308-2318`.
- `crates/substrate-store/src`, `crates/substrate-daemon/src/app`,
  `crates/substrate-daemon/tests/runtime_vectors.rs`. Inferred.
- `xtask/bundle-source`, `contracts/substrate-wire`, `xtask/src/bundle.rs`, `scripts/gate.sh`.
  Inferred from `AGENTS.md` § The gate.
- `AGENTS.md`, `README.md`, `CHANGELOG.md`, `website/docs`. Inferred.
