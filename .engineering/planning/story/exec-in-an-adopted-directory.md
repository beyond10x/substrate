---
format: aep.planning-md/3
id: story:exec-in-an-adopted-directory
kind: story
status: draft
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
revision: 2
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
