---
title: Contract surface
description: The current HTTP and WebSocket resource families exposed by Substrate.
---

# A small control plane with explicit recovery

Substrate serves JSON over HTTP and bounded WebSocket channels. Resource responses carry observed
state, and every response identifies the contract bundle and its digest in headers.

This page lists the implemented public families. Capability facts remain the authority for what a
particular daemon can serve.

## Machine

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/v1/machine` | deployment, driver generation, verified capabilities, limits, contract identity |

Query this route first, and do not infer support from a version number or deployment label.

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

The bounded Git routes in the next table compare against the host-private immutable
materialization baseline. They do not turn the workspace API into an arbitrary Git remote client.

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
The v1 directory page (`mode=directory`) and the v2 tree leave it out, as they leave out `.git`.
The tree never descends into it or counts it toward `limit_items`. Neither listing refuses because
of such an entry, and `workspace.path-escape` stays the answer for a path that escapes the
workspace.

Since bundle `0.23.0` (ADR 0043) both listings report the count of such entries in the
`x-b10x-omitted-entries` response header, sent only when the count is at least 1. On a v1 page it
is the directory's count, the same on every page. On the v2 tree it covers the part the walk
reached, so it is a lower bound when the walk reached `limit_items`. A `.git` entry, and a hidden
entry the request excludes, are not counted.

The tree still answers `truncated: true` for either of two causes: it reached `limit_items`, or it
omitted at least one such entry. The header tells them apart. `truncated: true` without the header
means the limit only, so raising the limit helps. With the header, at least one entry will stay
omitted whatever the limit. The Rust SDK's `Workspace::tree_with_omissions` and
`Workspace::read_directory_with_omissions` return the count beside the listing.

The workspace root is the path `.` on the v1 directory page (ADR 0052):
`GET /v1/workspaces/{workspace_id}/files/.?mode=directory` pages the root's immediate children
like any other directory. Send the segment as written: a client that removes dot segments reaches
`/files/` and gets 404. Every other file route refuses `.` with `workspace.path-escape`. A daemon
that predates the root page answers that code too, and a client may then fall back to the tree.

Release `0.27.0` advertises `substrate-wire/0.29.0` in `x-b10x-contract` and its inner
`bundle.json` SHA-256 in `x-b10x-contract-bundle-sha256`. The two headers are one claim. The signed
outer OCI manifest has a different digest, because it identifies the distribution package and not
the inner contract manifest.

[Bundle history](#bundle-history) lists what each bundle added. The session routes moved to
`/v1/sessions` in bundle `0.15.0`, and no `/v1/pipe-sessions` compatibility alias is served.

Frozen compatibility declarations and route/schema details are in the
[0.29.0 bundle](https://github.com/beyond10x/substrate/tree/0.27.0/contracts/substrate-wire/0.29.0).
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

Sessions are a development slice. Raw pipes remain the default, and PTY mode requires a bounded
initial window and a verified `sessions.pty` machine fact. Hosted attachment requires a
short-lived, one-use key-and-channel-bound authority. Unix attachment continues to use kernel peer
credentials.

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

## Bundle history

Earlier bundle directories remain immutable. A source contract promotion is not by itself a signed
daemon release. Consumers must pin and test the matching SDK/daemon revision. The SDK refuses an
older daemon's contract before issuing mutations.

### `0.28.0`

`0.28.0` adds no route and publishes the result bound of ADR 0053:

- Every capability snapshot states `operation.result-max-json-bytes`, the bytes of compact JSON
  the operation ledger records for one success result: 524,217. Its description says how a
  client counts the widest result of a request before sending it.
- A start of an exec, a session or a host process, and a v1 file write or delete, whose widest
  result exceeds that bound is `operation.result-limit` (429), and nothing starts. A request
  under the bound by that count is never refused for this cause.
- An operation that took effect and whose result the ledger cannot record ends with the error
  `operation.result-unrecorded` (500) and names its resource. Read the resource; do not repeat
  the mutation under a new operation id.

A client that validates snapshots against `0.27.0` must move to `0.28.0` to accept the fact.

### `0.27.0`

Release `0.22.0` was the first to advertise `substrate-wire/0.27.0`. It
adds no route and moves three bounds (issue #236):

- A sandbox's `apertures` list holds two to eight names, and the applied list's eighth aperture
  listens on `127.0.0.8`. A list of nine is `exec.apertures-too-many` (422).
- The `exec.egress-apertures` fact lists every aperture the deployment declared, up to twelve,
  where `0.26.0` and earlier bounded it by the run's four.
- A start may name up to eight `read_only_roots`, and a host publishes `exec.read-only-roots` as
  `{"max_roots": 8}`. Nine roots are `exec.read-only-root-invalid` (422).

All requests valid under `0.26.0` are answered the same way. A client that validates snapshots or
records against `0.26.0` must move to `0.27.0` to accept the longer lists.

### `0.26.0`

`0.26.0` adds no route. A capability snapshot may state what the serving confinement adapter
proved, never its name. It carries one optional fact per family, `confinement.family.process-run`
through `confinement.family.audit` in ADR 0044's order, and one per mechanism-neutral property,
such as `confinement.whole-tree-kill` or `confinement.paths-are-host-paths`. A fact is `true` or
absent, never `false`.

An exec or session start may carry `sandbox.requires`, a closed object with a `families` list and
a `facts` list of those names without their prefixes. An unknown name, a duplicate or an empty
object is `request.schema-invalid`.

The first required family the start's snapshot does not serve is `confinement.family-unserved`,
and the first required property it does not prove is `confinement.fact-unserved`. Both are `501`
with class `unserved`, recorded as refused and never dispatched. The applied record echoes
`requires`, and its `cgroup` is optional: present whenever a cgroup was applied, as on every host
run.

A host daemon publishes what its `linux` adapter proved, from the facts it already probes. It
always publishes `audit`, and with guarded workspace I/O it publishes `workspace`. On a host with
the exec confinement floor it publishes `process-run`, `process-watch`, `output-read`, `wait` and
`limits`, `filesystem-view` where read-only roots and files are proved, `network-egress` where an
egress aperture is declared, and the properties `whole-tree-kill`, `ipc-socket-allowlist` and
`ipc-signals-contained`. It never publishes `network-ingress`, and the API never names the
adapter.

### `0.25.0`

Release `0.16.1` was the first to advertise `substrate-wire/0.25.0`, which adds no route. A capability
snapshot's `driver` may now be `kubernetes` as well as `host`, and a driver may publish the
optional fact `confinement.no-egress` when a mechanism it verified keeps a workload off the
network.

Three refusals are new, each `501` with class `unserved` and nothing dispatched. An inline capsule
on a driver without `exec.inline-capsule` is `exec.inline-capsule-unserved`. A signal outside the
published `exec.signals` is `exec.signal-unserved`. An adopt on a driver that adopts no directory
is `workspace.adopt-unserved`. The host daemon's answers are unchanged: it still reports
`driver: host`, does not publish the new fact and raises none of the new codes.

The [Kubernetes namespace driver](../guides/deployment.md#kubernetes-namespace-driver) reports
`driver: kubernetes` and answers every adopt `workspace.adopt-unserved`. In releases 0.16.1 and 0.17.0
its probe proved neither the four Linux confinement facts nor `confinement.no-egress`. It refused
every exec start `exec.sandbox-unavailable`, and the two exec codes were not reached. From 0.18.0 it
admits an exec on the confinement its probe observed.

### `0.24.0`

`0.24.0` added one route, the per-process
event stream of ADR 0036 S12: `GET /v1/processes/{process_id}/events`, a WebSocket under the
`process` scope. Its frames interleave the record's `process.*` events and its output lines in one
order, report a `gap` for log bytes the bound discarded, and end with exactly one `ended` item
naming `ended_by`. Each frame carries an opaque cursor to resume from.

The event union gains seven `process.*` variants, so a client that validates events against
`0.23.0` must move to `0.24.0`. A ninth concurrent stream of one record, a subject's 513th or a
deployment's 513th, live or ended records alike, is refused `process.events-readers-exceeded`
(429).

### `0.23.0`

`0.23.0` adds no route and publishes the read-only roots the wire has always admitted (ADR 0010).
An exec or session start may name `read_only_roots`, at most four
`{"host_path": <path>, "mount": <path>}` entries, each path absolute and canonical. A host
publishes `exec.read-only-roots`, `{"max_roots": 4}`, only when it is confined and resolves paths
without following a symlink. The applied record lists each bound root as declared.

No refusal is new: `exec.read-only-root-invalid`, `exec.read-only-root-absent` and
`exec.read-only-root-not-a-directory` (422) are now published, each with a refusal vector. A
start naming a root on a host without the fact is refused `exec.sandbox-unavailable` (501) before
dispatch, after the checks of the start's other confinement members.

The exec applied record now also admits `secret_slots`, which `0.5.0` to `0.22.0` wrongly reject,
and `read_only_files`, which `0.18.0` to `0.22.0` wrongly reject. A client that validates
capability snapshots or resources against `0.22.0` must move to `0.23.0` to accept the new fact
and record members.

The session start's `exec` object now also publishes `scratch`, `measurements` and
`workspace_access`, each the same schema as on exec start. A session has always accepted and
applied them. Both starts now publish the refusals those three members have always raised, each
checked against its `substrate_wire` constant. The daemon refuses a null `scratch`, a storage
`max_bytes` that is not a multiple of 1024 and a repeated measurement as `request.schema-invalid`
(422), as the schemas always did.

### `0.22.0`

`0.22.0` added ADR 0038: an exec or session start may name `writable_roots`, at most eight
writable mounts outside `/workspace`. An entry is exactly one of
`{"host": <name>, "mount": <path>}`, a directory the operator declared by name, or
`{"workspace": <workspace id>, "mount": <path>}`, a Git checkout workspace of the caller. A
request never carries a host path.

A mount is absolute and canonical, never `/`, and neither is, lies beneath nor contains a system
path, `/workspace`, `/runtime`, `/scratch`, another writable root or a read-only root or file. Each
of those rules has its own named refusal (422), checked before any capability.

A host publishes `exec.writable-roots.host`, the declared names, and `exec.writable-roots.git`
only after it proved a root bound by descriptor is writable. A start naming a kind or name no fact
serves is `exec.writable-roots-host-unserved` or `exec.writable-roots-git-unserved` (501), and the
message names the root.

The applied record lists each bound root with its kind, name or workspace id, host path and mount,
and a `workdir` may lie beneath a writable root. A client that validates capability snapshots or
resources against `0.21.0` must move to `0.22.0` to accept the new facts and record member.

### `0.21.0`

`0.21.0` added design 26. An exec or session start may name `workdir`, the directory the program
starts in: an absolute, lexically normal path of at most 4096 bytes at or beneath `/workspace`, a
declared read-only root mount, `/scratch` when scratch is requested, or `/runtime` when a capsule
is carried.

A host that has proved it can apply one publishes the `exec.workdir` fact and refuses an absent or
symlinked directory as `exec.workdir-missing` before anything runs. Without the fact a start that
names one is `exec.workdir-unserved`.

A sandbox may name `apertures`, a list of two to four distinct declared apertures, instead of one
`aperture`. An applied list is an array whose elements state the loopback address each aperture
listens on. A Linux host refuses a list in which two apertures declare one host name, or any but
the first declares `localhost`, as `exec.aperture-host-listed-twice`, because the child could
reach only one of them by that name.

A client that validates capability snapshots or resources against `0.20.0` must move to `0.21.0`
to accept the new fact and record forms.

### `0.20.0`

`0.20.0` added the host-process family (ADR 0036): seven routes under `/v1/processes` to start,
list, read, stop, stop by owner session, renew the lease of and tail the log of an unconfined
process the daemon supervises in a cgroup of its own. Every record says `profile: host` and
`confinement: none`.

A daemon publishes the optional `process.host` fact only when its delegated cgroup subtree can
hold process cgroups, and serves these routes only on the personal Unix socket to a peer of its
own uid. A client that validates capability snapshots or resources against `0.19.0` must move to
`0.20.0` to accept the new fact and record.

### `0.19.0`

`0.19.0` added the explicit `workspace.adopt`
operation at `POST /v1/workspaces/adopt`, which serves a directory the operator placed under the
workspace root, and two required read-only members on the workspace resource: `origin` and
`root_name`. Workspace ids keep their `ws_` form. A client that validates workspaces against
`0.18.0` must move to `0.19.0` to accept the new members.

### `0.18.0`

`0.18.0` made the
`exec.resource-usage.block_io` fact a boolean: a host without the `io` controller publishes it
`false` and omits `io_read_bytes` and `io_write_bytes` from every usage record instead of refusing
execution usage. It also added the optional `read_only_files` start field, which binds single host
files read-only, together with the `exec.read-only-files` fact and that field's named refusals.

### `0.17.0`

`0.17.0` added explicit session capture selection, content-free output counters,
capture-policy refusals and attachment ceilings. Existing requests retain recorded output.
