---
format: aep.planning-md/3
id: story:exec-file-changes-are-recorded
kind: story
status: active
title: An exec's effective file changes are recorded and attributed to that exec
summary: 'In process: per run on the linux adapter, path, op, before and after digests and diffable content from tree digests before and after; unavailable with a named reason when another writer shared the mount. ADR and model first.'
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
  path: adr
- confidence: inferred
  path: adr/README.md
- confidence: inferred
  path: crates/substrate-host/src/confinement/facts.rs
- confidence: inferred
  path: crates/substrate-host/src/confinement/linux.rs
- confidence: cited
  path: crates/substrate-host/src/confinement/mod.rs
- confidence: inferred
  path: crates/substrate-host/src/confinement/profile.rs
- confidence: inferred
  path: crates/substrate-host/src/probe.rs
- confidence: inferred
  path: crates/substrate-host/src/process.rs
- confidence: cited
  path: crates/substrate-host/tests/confinement-public-items.txt
- confidence: inferred
  path: spec/confinement
- confidence: inferred
  path: spec/confinement-suite.json
- confidence: inferred
  path: website/docs/concepts/adapters.md
revision: 16
transitions:
- {from: "draft", to: "proposed", at: "2026-11-21T03:08:32Z", actor: "human:timo", revision: 14, decided_on: {"recorded":{"review_outcome":5}}, correlation: "sdc-accepts-usage-and-file-changes-2026-11-21"}
- {from: "proposed", to: "active", at: "2026-11-21T03:08:32Z", actor: "human:timo", revision: 15, decided_on: {"recorded":{"review_outcome":5}}, correlation: "sdc-accepts-usage-and-file-changes-2026-11-21"}
---
# Story: An exec's effective file changes are recorded and attributed to that exec

## Outcome

On the in-process path, every run on the `linux` adapter whose profile asks for it has a change
record: the files it changed
in its workspace and declared writable roots, each with path, operation (create, modify, delete,
rename), the content digest before and after, and content an embedder can diff. The record is
attributed to the run whoever wrote the file inside it (an agent's tool, a script, a formatter, a
linter). It lists only that run's changes. A run that shared a writable mount with another writer
while it ran has an unavailable record with a named reason instead. The wire is
`story:exec-file-change-record-on-the-wire`.

Three words, used the same way throughout: a run that changed nothing has an **empty** record; a
path that ends as it started is **not listed**; a run whose changes cannot be attributed has an
**unavailable** record with a named reason.

## Where the requirement comes from

Relayed by session `sdc` on 2026-11-21 as Timo's requirement of the same day. The SDC prototype
records changes itself today, with inotify and attribution by time window
(`sdc/docs/fs-changes.md` in the `sdc` repository, record `sdc.fs-change/1`). That recorder
retires once Substrate has the record.

## Decided

Session `sdc` decided on 2026-11-21, under
`approval-record:sdc-decides-plan-and-design-2026-11-21`:

- The mechanism is a digest of each writable mount's tree before the exec starts and after it
  ends.
- One writer per workspace. SDC runs one exec at a time in a workspace, and Loom's planned
  concurrency gives each story its own work tree, so concurrent execs write different workspaces.
- When a second exec wrote the same workspace during an exec, no record is published for it. A
  digest pair is never wrong.
- No fallback to a time window.
- ~~This story is worked after `story:run-observation-carries-usage`.~~ Replaced on the same
  evening, see below.

Not chosen: an overlay upper directory per exec (exact for two writers of one workspace, but
concurrent execs stop seeing each other's writes), and fanotify with the writer's pid (inferred:
the marks that report another process's pid need `CAP_SYS_ADMIN`, which the daemon does not hold).

## The consumer's requirements, 2026-11-21 evening

Session `runbook` (the session formerly named `sdc`) accepted the story as written and added, as
the consumer whose `watch/` crate this record replaces
(`approval-record:order-of-work-after-0-23-0-2026-11-21`):

- **The unit is the run.** A bound program (a check, a build, a test run) is one action, one
  exec and one run. An agent step is one long-lived process for one action, and its tool calls
  happen inside it: all its changes are one record, attributed to that run. That is enough for
  the consumer; "what did this action change" is the question the record answers.
- **The record carries the run's start and end instants.**
- **Each listed path carries the file's modification time as read in the end walk**, so a
  consumer can place a change between two instants of its own log. It is the file system's
  time, marked as such and never called an observed instant, and it is absent for a delete. The
  consumer's condition: the field costs no more than the `stat` the end walk already makes; if it
  costs more, it is dropped and the consumer is told. The ADR states what the field is on a
  rename and what a writer inside the run can do to it (a process can set a file's times).
- The stakeholder confirmed the acceptance line for two runs that write one workspace: both
  records unavailable with a named reason.
- **Order.** This story and `story:exec-file-change-record-on-the-wire` are worked after
  `story:executable-deny-list` and the two gate stories, and before the limit and usage stories.

## Why

Verified at `main` `adfde4020`:

- Substrate records a file change only when its own file routes made it: the events
  `workspace.file-written` and `workspace.file-deleted`
  (`crates/substrate-store/src/operations.rs:854-855`). A write by a process inside an exec
  produces no record.
- `GitChangeSet` compares a Git workspace with its materialization commit
  (`crates/substrate-wire/src/lib.rs:1502-1515`). It is per workspace, not per exec, and exists
  only for a Git workspace.
- `RunObservation` and `AuditRecord` carry no file change
  (`crates/substrate-host/src/confinement/mod.rs:280-324`, `:696-706`).
- Attribution by time window cannot separate two processes that write in the same window. Only
  the layer that starts the process and builds its filesystem view can attribute exactly.

## What changes

1. **An ADR and the model first** (`AGENTS.md` invariant 8; a new noun is modelled before its
   stories). The ADR records the decided mechanism and decides:
   - how an overlap is detected: which starts count as a second writer of a mount (another exec
     with write access to it, a session, a write through Substrate's own file routes), and the
     named reason an unavailable record carries;
   - what the host cannot see. A process outside Substrate that writes the workspace directory on
     the host is not a start Substrate knows; the ADR states that limit;
   - the record: its fields, how a rename and an atomic replace are reported from two tree
     digests, the bound on paths per run and on content bytes kept, what is recorded for a file
     over the bound or a binary file, and how long content is kept;
   - which mounts are covered: the workspace and declared writable roots (ADR 0038). Whether
     `/scratch` is covered, and which paths are left out (`.git` internals, for one);
   - the audit sink: a new `AuditRecord` variant written when the record is complete, carrying
     the run and the count of changed paths and never file content. It is not a member of the
     `run_ended` record that `docs/design/36-limit-hits.md` owns; the ADR states its size bound;
   - the fact the adapter proves and the probe behind it;
   - how the before side's content is available. Two digests do not hold it: the content of a
     file as it was at the start must be kept somewhere until the run ends;
   - the profile member that asks for the record, and the named answer the adapter API gives for
     a run whose profile did not ask;
   - the cost: what two tree digests and keeping the before side add to a run's start and end on
     a workspace of a stated size, measured on a workstation and recorded in the planning store,
     and the size bound above which the record is unavailable with a named reason;
   - the wire shape, for `story:exec-file-change-record-on-the-wire`.
2. The ESS model gains the record as an entity related to the run, under `spec/confinement`,
   validated with `ess specify validate` and held by `cargo xtask check-ess`.
3. The host driver digests the writable mounts before and after every exec that asks for the
   record. It keeps the record and its content beside the exec, for the retention the ADR sets,
   and exposes both through a host-driver call keyed by the exec's id. The record is not a member
   of `ExecObservation`, so no existing `ExecObservation` literal changes.
4. The `linux` adapter reads the record from that host-driver call, exposes it through the adapter
   API and writes the audit record. `story:exec-file-change-record-on-the-wire` has the daemon
   read the same call; the daemon keeps no second copy.

## Acceptance

In the delegated lane, two runs on the in-process `linux` adapter that run at the same time in two
workspaces each create, modify, delete and rename files, and each run's change record, read
through the adapter API, lists exactly its own changes with the before and after digests; before
this story a run has no change record.

Checks:

- The ADR is accepted and indexed in `adr/README.md`, and the model validates, before any code.
- Two runs that write one workspace at the same time: each record is unavailable with the named
  reason, and neither lists a change.
- A run that changed nothing has an empty record. A path a run wrote and restored to its starting
  content is not listed.
- A change in a declared writable root is listed with that root named.
- A change made through a tool, a shell script and a formatter inside one run all land in that
  run's record.
- Content for both sides of a modified text file is readable under the declared bound, and a file
  over the bound is listed with digests and marked as over the bound.
- A workspace above the size bound the ADR sets has an unavailable record with the named reason,
  and the run itself runs.
- The profile's audit sink receives one record of the new variant per run with an available
  record.
- A run whose profile did not ask for the record: the adapter API answers the named answer the ADR
  gives for "not requested", and the profile renders and runs as before this story.
- A profile that asks for the record on `local`, `macos`, `oci` or `pod`, or on a `linux` host
  whose probe did not prove the fact, is refused `confinement.fact-unserved` naming the new fact,
  and nothing runs (invariant 3).
- `crates/substrate-host/tests/confinement-public-items.txt` is regenerated in the same change.

## Boundaries

- No wire route, event or bundle: `story:exec-file-change-record-on-the-wire`.
- No diff rendering, line counts or syntax highlighting. Substrate keeps digests and content; the
  client computes a diff.
- No attribution below the run: which tool call inside a run wrote a file stays the consumer's.
- No record of reads.
- No record for two writers of one workspace.
- No adapter other than `linux`, and `systemd` around `linux`.
- Changes made through Substrate's own file routes keep their existing events.

## Order

Informed by `epic:runs-are-governed-and-limit-hits-are-events` and not part of it. After
`story:executable-deny-list`, `story:gate-test-step-ends-within-two-minutes` and
`story:planning-only-pull-requests-skip-build-and-test`, and before
`story:run-observation-names-the-limit-that-ended-it`, as session `runbook` decided on
2026-11-21 (`approval-record:order-of-work-after-0-23-0-2026-11-21`). It shares
`crates/substrate-host/src/confinement/mod.rs` and `crates/substrate-host/src/process.rs` with
the limit and usage stories, which now take this story's `main`. The
epic's later drafts `story:executable-lists-on-oci-and-pod` and
`story:exec-start-carries-executable-lists` are ordered after this story because they share
`spec/confinement/confinement.yaml`, `crates/substrate-host/src/process.rs` and
`crates/substrate-host/src/probe.rs` with it.

## Scope

- `adr` (a new ADR), `adr/README.md`. Inferred.
- `spec/confinement`, `spec/confinement-suite.json`. Inferred.
- `crates/substrate-host/src/process.rs`, `crates/substrate-host/src/probe.rs`. Inferred: the
  filesystem view and the probe live there.
- `crates/substrate-host/src/confinement/facts.rs`. Inferred: the new fact.
- `crates/substrate-host/src/confinement/mod.rs`. Cited: `:280-324`, `:696-706`.
- `crates/substrate-host/src/confinement/linux.rs`,
  `crates/substrate-host/src/confinement/profile.rs`. Inferred.
- `crates/substrate-host/tests/confinement-public-items.txt`. Cited: `:1-3`.
- `website/docs/concepts/adapters.md`. Inferred: the adapter fact table `cargo xtask check-docs`
  holds to the spec.
