---
format: aep.planning-md/3
id: decision-blocker:exec-in-adopted-directory-contract-route
kind: decision-blocker
status: open
title: Contract route for an exec in an adopted directory
relations:
- blocks: story:exec-in-an-adopted-directory
revision: 1
---
## Question

How does an exec reach an adopted existing directory (`story:exec-in-an-adopted-directory`)?
Every released bundle publishes the workspace identifier as `^ws_[A-Za-z0-9]+$`
(`contracts/substrate-wire/0.18.0/schemas/common.json:294-296`), and the daemon's exec admission
enforces the same prefix (`crates/substrate-daemon/src/app/operations.rs:416-419`). Any route that
lets a socket client start an exec in a directory not named `ws_…` changes the wire, so it needs a
successor bundle under invariant 6 and an ADR before code under invariant 8.

## Options

- A: successor bundle `0.19.0` widens `workspace-id` to the single non-escaping path component
  `validate_root_name` already admits; an ADR records the pattern and its refusal. Smallest change;
  every route that names a workspace accepts adopted names.
- B: successor bundle adds an explicit adopt operation that records a host directory name against a
  minted `ws_` id; the identifier pattern stays. A durable record of each adoption, one new route
  and its refusals.
- C: no wire change; adoption stays an in-process `Driver` capability for embedders. A socket
  client gets nothing.

## Recommendation

A, unless a durable adoption record is wanted for audit, in which case B. Either is an ADR-level
change and is not decided here.
