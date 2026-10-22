---
format: aep.planning-md/3
id: story:exec-in-an-adopted-directory
kind: story
status: implemented
title: An exec runs in an adopted existing directory
relations:
- decomposes: epic:remote-serving
scope:
- confidence: cited
  path: contracts/substrate-wire
- confidence: cited
  path: crates/substrate-daemon/src/app/operations.rs
- confidence: cited
  path: xtask/bundle-source
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-10-17T05:05:36Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-17T05:05:36Z", actor: "human:timo", revision: 4}
- {from: "active", to: "implemented", at: "2026-10-21T23:37:41Z", actor: "human:timo", revision: 6, decided_on: {"recorded":{"test_result":1,"review_outcome":1}}}
---
# Story: An exec runs in an adopted existing directory

## Context

A workspace root may be a directory the operator already owns: `validate_root_name`
(`crates/substrate-host/src/fs.rs`) accepts any single non-escaping path component, while
`create_workspace` still mints `ws_…` (`crates/substrate-daemon/src/app/service.rs:129`). Over the
socket an adopted directory can be read and written, but `exec.start` refuses it: the admission
shape check requires `input.workspace.starts_with("ws_")` followed by ASCII alphanumerics
(`crates/substrate-daemon/src/app/operations.rs:416-419`). `STATUS.md` records adoption as "half
done" for exactly this predicate.

A client embedding Substrate to run a repository's gate in its existing checkout (a managed
worktree adopted as the workspace) needs the other half: the daemon runs an exec in that directory.
More than one core per exec is not required; `story:configurable-exec-cpu-ceiling` already shipped
that.

## Why this is a contract change

Every released bundle publishes the workspace identifier as `^ws_[A-Za-z0-9]+$`
(`contracts/substrate-wire/0.18.0/schemas/common.json:294-296`, the `workspace-id` definition),
and 20 schemas under `contracts/substrate-wire/0.18.0/schemas/` reference it. Widening the daemon's
predicate without a successor bundle would make the daemon accept an exec start the published
schema rejects, so a bundle-only client could neither send it nor validate the answer. Invariant 6
forbids editing `0.18.0`; invariant 8 requires a design document or an ADR before code. The route
is recorded in `decision-blocker:exec-in-adopted-directory-contract-route`, which blocks this story.

## Acceptance

- A successor bundle publishes the identifier an adopted workspace is addressed by on the exec-start
  route (and on every route that names a workspace), and the daemon admits exactly that set.
- A runtime vector creates a directory under the workspace root without the `ws_` prefix, adopts it,
  starts an exec in it through the shipped daemon and observes the exec's output and exit; the same
  vector shows a name outside the published pattern refused before dispatch with a named code.
- The confinement floor is unchanged for an adopted workspace: the delegated lane runs the same
  isolation cases against it.

## Notes

Shapes to weigh in the decision, none taken:

- widen `workspace-id` to the single-component name `validate_root_name` already admits (a
  pattern that excludes `/`, `.`, `..`, NUL and the hidden prefix), in a successor bundle;
- keep `ws_` identifiers and add an explicit adopt operation that maps a host directory name to a
  minted `ws_` id, so the wire pattern does not change and the adoption is a durable record;
- leave the predicate and serve adoption only to in-process embedders through the `Driver` trait,
  which is not the wire and gives a socket client nothing.

## State after 0.7.17

Shipped in 0.7.17 (ADR 0035, bundle `substrate-wire/0.19.0`): the explicit `workspace.adopt` route,
the minted `ws_` id with `origin` and `root_name` on the resource, and its pre-dispatch refusals.
`crates/substrate-daemon/tests/adopt.rs` proves adopt, exec, a name outside the set refused and
recorded, and destroy forgetting the row, all through the in-process app and a fixture driver.

Still open, so the story stays active:

- the second acceptance line names the **shipped daemon**: no runtime vector in
  `crates/substrate-daemon/tests/runtime_vectors.rs` adopts a directory and runs an exec in it; the
  delegated lane counts only the `workspace.adopt` registry probe;
- the third acceptance line: the delegated lane runs no isolation case against an adopted
  workspace.
