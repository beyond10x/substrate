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

The host serves empty workspaces and conditionally serves Git materialization. A Git request names
a deployment-configured HTTPS source, an authorized opaque locator, the provider branch, an exact
commit and a depth from 1 through 50. It requires the `workspace.git` fact and transient source
authority. The resulting working tree has detached HEAD at that commit. Its `.git` directory is
usable in confined terminals but hidden from file and tree APIs.

The bounded Git routes below compare against the host-private immutable materialization baseline.
They do not turn the workspace API into an arbitrary Git remote client.

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

A directory entry the contract cannot name is omitted from both listings: an entry that is not a
regular file, directory or symbolic link (a FIFO, socket or device), or whose name is not UTF-8.
The v1 directory page (`mode=directory`) and the v2 tree leave it out, as they leave out `.git`,
and the tree never descends into it or counts it toward `limit_items`. The v1 page gives no signal
that it omitted an entry. The v2 tree answers `truncated: true` for either of two causes: it
reached `limit_items`, or it omitted at least one such entry, so a retry with a higher limit after
an omission still answers `truncated: true`. Neither listing refuses because of such an entry;
`workspace.path-escape` stays the answer for a path that escapes the workspace.

Release `0.7.3` advertises `substrate-wire/0.16.0` in `x-b10x-contract` and its inner
`bundle.json` SHA-256 in `x-b10x-contract-bundle-sha256`. The two headers are one claim. The signed
outer OCI manifest has a different digest because it identifies the distribution package rather
than the inner contract manifest.

The current release is `0.7.3` with development bundle `0.16.0`. That bundle adds the
closed Git source request and two Git observation routes. Earlier additions now served include
PTY sessions, metrics, hard storage quotas, scoped workspace write authority and hosted admission.
The session routes moved to `/v1/sessions` in bundle `0.15.0`; no `/v1/pipe-sessions`
compatibility alias is served.

Frozen compatibility declarations and route/schema details are available in the
[0.16.0 bundle](https://github.com/beyond10x/substrate/tree/0.7.3/contracts/substrate-wire/0.16.0).
See [system model and derivation](../concepts/model.md) for which artifacts are generated and
[status](../status.md) for release maturity.

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
a snapshot after a history gap. A reconciliation snapshot contains observed resource state, not a
restorable copy of workspace files.

## Identity and operation IDs

Resource IDs are opaque and server-minted. Mutation operation IDs are caller-minted and stable
across retry. Resource and operation lookup stays within the authenticated subject namespace.

Read [operations and observations](../concepts/operations.md) for retry semantics and
[status](../status.md) for the implementation boundary.

Development source now advertises `substrate-wire/0.22.0`. It adds no route. An exec or session
start may name `writable_roots`, at most eight writable mounts outside `/workspace` (ADR 0038). Each
entry is exactly one of `{"host": <name>, "mount": <path>}`, a directory the operator declared by
name, or `{"workspace": <workspace id>, "mount": <path>}`, a Git checkout workspace of the caller; a
request never carries a host path. A mount is absolute and canonical, never `/`, and neither is, lies
beneath nor contains a system path, `/workspace`, `/runtime`, `/scratch`, another writable root or a
read-only root or file; each of those rules has its own named refusal (422), checked before any
capability. A host publishes `exec.writable-roots.host`, the declared names, and
`exec.writable-roots.git` only after it proved a root bound by descriptor is writable; a start
naming a kind or name no fact serves is `exec.writable-roots-host-unserved` or
`exec.writable-roots-git-unserved` (501), and the message names the root. The applied record lists
each bound root with its kind, name or workspace id, host path and mount, and a `workdir` may lie
beneath a writable root. A client that validates capability snapshots or resources against `0.21.0`
must move to `0.22.0` to accept the new facts and record member.
`0.21.0` added design 26. An exec or session
start may name `workdir`, the directory the program starts in: an absolute, lexically normal path
of at most 4096 bytes at or beneath `/workspace`, a declared read-only root mount, `/scratch` when
scratch is requested, or `/runtime` when a capsule is carried. A host that has proved it can apply
one publishes the `exec.workdir` fact and refuses an absent or symlinked directory as
`exec.workdir-missing` before anything runs; without the fact a start that names one is
`exec.workdir-unserved`. A sandbox may name `apertures`, a list of two to four distinct declared
apertures, instead of one `aperture`; an applied list is an array whose elements state the loopback
address each aperture listens on. A Linux host refuses a list in which two apertures declare one
host name, or any but the first declares `localhost`, as `exec.aperture-host-listed-twice`, because
the child could reach only one of them by that name. A
client that validates capability snapshots or resources against `0.20.0` must move to `0.21.0` to
accept the new fact and record forms.
`0.20.0` added the host-process family
(ADR 0036): seven routes under `/v1/processes` to start, list, read, stop, stop by owner session,
renew the lease of and tail the log of an unconfined process the daemon supervises in a cgroup of
its own. Every record says `profile: host` and `confinement: none`. A daemon publishes the optional
`process.host` fact only when its delegated cgroup subtree can hold process cgroups, and serves
these routes only on the personal Unix socket to a peer of its own uid. A client that validates
capability snapshots or resources against `0.19.0` must move to `0.20.0` to accept the new fact
and record.
`0.19.0` added the explicit `workspace.adopt`
operation at `POST /v1/workspaces/adopt`, which serves a directory the operator placed under the
workspace root, and two required read-only members on the workspace resource: `origin` and
`root_name`. Workspace ids keep their `ws_` form. A client that validates workspaces against
`0.18.0` must move to `0.19.0` to accept the new members.
`0.18.0` made the
`exec.resource-usage.block_io` fact a boolean: a host without the `io` controller publishes it
`false` and omits `io_read_bytes` and `io_write_bytes` from every usage record instead of refusing
execution usage. It also added the optional `read_only_files` start field, which binds single host
files read-only, together with the `exec.read-only-files` fact and that field's named refusals.
`0.17.0` added explicit session capture selection, content-free output counters,
capture-policy refusals and attachment ceilings; existing requests retain recorded output. Earlier bundle directories remain immutable. This source contract promotion
is not a new signed daemon release; consumers must pin and test the matching SDK/daemon revision.
The SDK refuses an older daemon's contract before issuing mutations.
