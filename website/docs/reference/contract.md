---
title: Contract surface
description: The current HTTP and WebSocket resource families exposed by Substrate.
---

# A small control plane with explicit recovery

Substrate serves JSON over HTTP and bounded WebSocket channels. Resource responses carry observed
state, and every response identifies the contract bundle and its digest in headers.

This page summarizes the currently implemented public families. Capability facts remain the
authority for what a particular daemon can serve.

## Machine

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/v1/machine` | deployment, driver generation, verified capabilities, limits, contract identity |

Query this first. Do not infer support from a version number or deployment label.

## Workspaces

| Method | Path | Purpose |
|---|---|---|
| `POST` | `/v1/workspaces` | create a workspace through a keyed operation |
| `GET` | `/v1/workspaces/{workspace_id}` | read observed workspace state |
| `DELETE` | `/v1/workspaces/{workspace_id}` | destroy a workspace through a keyed operation |
| `GET` | `/v1/workspaces/{workspace_id}/files/{*path}` | read a bounded file range |
| `PUT` | `/v1/workspaces/{workspace_id}/files/{*path}` | atomically replace a file |
| `DELETE` | `/v1/workspaces/{workspace_id}/files/{*path}` | delete a guarded path |
| `POST` | `/v1/workspaces/{workspace_id}/lease/renew` | renew workspace liveness |

The current host slice serves empty workspace creation. Git source materialization is absent.

### Development v2 byte plane

Bundle `0.9.0` declares the already-served v2 workspace byte plane. It adds no new resource family:
it gives file operations closed request shapes for bounded directory reads, byte replacement,
edits, and patches.

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/v2/workspaces/{workspace_id}/files/{*path}` | read bounded file bytes |
| `GET` | `/v2/workspaces/{workspace_id}/tree` | read a bounded directory tree |
| `GET` | `/v2/workspaces/{workspace_id}/git/baseline/{*path}` | read one bounded file at the materialized commit |
| `GET` | `/v2/workspaces/{workspace_id}/git/changes` | compare the index/worktree with the materialized commit under item and byte bounds |
| `PUT` | `/v2/workspaces/{workspace_id}/files/{*path}` | atomically replace file bytes |
| `POST` | `/v2/workspaces/{workspace_id}/file-edits/{*path}` | apply one bounded positional edit |
| `POST` | `/v2/workspaces/{workspace_id}/file-patches/{*path}` | apply one bounded patch |

Current development source advertises `substrate-wire/0.16.0` in `x-b10x-contract` and its inner
`bundle.json` SHA-256 in `x-b10x-contract-bundle-sha256`. The two headers are one claim. The signed
outer OCI manifest has a different digest because it identifies the distribution package rather
than the inner contract manifest.

Bundle `0.10.0` succeeds `0.9.0` without adding a route. It adds a `pty` session mode with a
required bounded terminal window, a closed resize-capable frame vocabulary, and a capability fact
that is present only after the host proves terminal allocation and resize behavior. Without that
fact, PTY start is refused and is never served as raw pipes.

Bundle `0.11.0` succeeds `0.10.0`, preserves its 31 routes, and adds `GET /v1/metrics` plus
`GET /v1/metrics/stream`. It also declares hard `/workspace` and `/scratch` quota requests and the
exact, explicitly requested resource-usage observation. Bundle `0.11.0` exists for development
consumers to pin and verify before the server claims it; its existence is not a stability or
compatibility promise.

Bundle `0.12.0` has an already-frozen compatibility block that names `0.10.0`, preserves 31 routes,
and adds the two metrics routes. Before promotion, an additional gate proves that its complete
33-route declarations match `0.11.0` and that the quota and metrics behavior from `0.11.0` remains
present beside exact read-only or scoped workspace write authority. This one recorded lineage
bridge does not make the bundle stable or make any earlier bundle mutable.

Bundle `0.13.0` directly succeeds `0.12.0`, preserves all 33 routes and adds none. It declares the
hosted production-admission profile: exact Identity audience, five-minute online authority
resolution, the `observe`/`workspaces`/`exec` route mapping and the four `auth.*` refusals a remote
client can receive. It remains a development contract.

Bundle `0.14.0` adds the hosted-only attachment-authority mint route and binds each one-use bearer
to an Ed25519 key and the accepting TLS 1.3 channel. Bundle `0.15.0` then performs one deliberate
pre-1.0 break: its eight session addresses move from `/v1/pipe-sessions` to `/v1/sessions`, with no
redirect or compatibility alias. Operation ids and request/result schemas stay the same.

Bundle `0.16.0` succeeds `0.15.0` additively. It closes the Git workspace source shape around a
deployment-configured HTTPS source, opaque locator, provider branch, exact commit and depth 1–50;
adds the conditional `workspace.git` fact; and adds the two bounded Git observation routes above.
The transient source authority is an HTTP header and is absent from every contract JSON document.

### Metrics

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/v1/metrics` | refresh one exec or workspace usage observation |
| `GET` | `/v1/metrics/stream` | receive latest-wins live exec samples over WebSocket |

## Execs

| Method | Path | Purpose |
|---|---|---|
| `POST` | `/v1/execs` | start a bounded exec through a keyed operation |
| `GET` | `/v1/execs/{exec_id}` | read observed exec state |
| `GET` | `/v1/execs/{exec_id}/output` | read persisted bounded output |
| `POST` | `/v1/execs/{exec_id}/signal` | signal a running exec |
| `POST` | `/v1/execs/{exec_id}/lease/renew` | renew exec liveness |
| `DELETE` | `/v1/execs/{exec_id}` | retire an exec |

Exec start is served only when the daemon verified its complete host confinement floor.

## Sessions

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/v1/sessions` | read session capability information |
| `POST` | `/v1/sessions` | start a leased raw-pipe or PTY session |
| `GET` | `/v1/sessions/{session_id}` | read observed session state |
| `GET` | `/v1/sessions/{session_id}/attach` | attach the one bounded WebSocket channel |
| `POST` | `/v1/sessions/{session_id}/attachment-authorities` | mint one bounded hosted WSS attachment authority |
| `POST` | `/v1/sessions/{session_id}/signal` | signal the underlying process |
| `POST` | `/v1/sessions/{session_id}/lease/renew` | renew session liveness |
| `DELETE` | `/v1/sessions/{session_id}` | retire a session |

Sessions are a development slice. Raw pipes remain the default; PTY mode requires a bounded initial
window and a verified `sessions.pty` machine fact. Hosted attachment requires a short-lived,
one-use key-and-channel-bound authority; Unix attachment continues to use kernel peer credentials.

## Recovery and events

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/v1/ops/{operation_id}` | reconcile one caller-minted operation |
| `GET` | `/v1/events` | page through retained typed events |
| `GET` | `/v1/events/stream` | follow the same event sequence over WebSocket |
| `POST` | `/v1/reconciliation-snapshots` | create a bounded state barrier |
| `GET` | `/v1/reconciliation-snapshots/{snapshot_id}` | read the barriered recovery view |

Event replay is bounded by the retention advertised in machine facts. Consumers must reconcile from
a snapshot after a history gap.

## Identity and operation IDs

Resource IDs are opaque and server-minted. Mutation operation IDs are caller-minted and stable
across retry. Resource and operation lookup stays within the authenticated subject namespace.

Read [operations and observations](../concepts/operations.md) for retry semantics and
[status](../status.md) for the implementation boundary.
