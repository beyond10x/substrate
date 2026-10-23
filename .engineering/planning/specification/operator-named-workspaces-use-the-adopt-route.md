---
format: aep.planning-md/3
id: specification:operator-named-workspaces-use-the-adopt-route
kind: specification
status: draft
title: Operator-named workspaces reach exec through workspace.adopt
revision: 1
---
# Operator-named workspaces reach exec through `workspace.adopt`

Harness's dependency blocker `substrate-daemon-admits-operator-named-workspaces` asks the daemon to
admit an operator-named workspace where `exec.start` takes a workspace id. Substrate already serves
that need without a wire change: a client calls `POST /v1/workspaces/adopt` (ADR 0035,
`crates/substrate-daemon/src/app/routes.rs:61`, handler `workspace_adopt` in
`crates/substrate-daemon/src/app/workspaces.rs`) with `root_name` set to the operator's directory
name under the daemon's workspace root, and receives a Ready workspace row owned by the calling
subject with a minted `ws_…` id, the given `root_name` and `origin: adopted`. Every later route,
`exec.start` and session start included, addresses the workspace by that `ws_…` id, and the
exec runs in the operator's directory; destroying the row leaves the files. The id check
the blocker cites (`validate_exec_input`, `crates/substrate-daemon/src/app/operations.rs`) is ADR
0035 decision 2 (`adr/0035-*.md`, "The wire `workspace-id` stays `^ws_[A-Za-z0-9]+$`"), which
withdrew ADR 0034's widening of the id to operator names; admitting a raw name as the id again is a
wire-contract change that needs a new ADR and a successor bundle, and is not planned. The runtime
vectors prove the route end to end (`check_adopted_workspace` in
`crates/substrate-daemon/tests/runtime_vectors.rs`: adopt, exec by root name refused, exec by the
minted id served on the delegated lane, destroy keeps the files). Harness's fix is on its side: adopt
once per operator directory, keep the returned `ws_…` id, and pass that id to exec.
