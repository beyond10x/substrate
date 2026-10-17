---
status: accepted
date: 2026-10-17
---

# ADR 0036: host processes are a supervised resource family

## Context

Callers that run agent sessions on a workstation start long-lived processes: development servers,
headless browsers, file watchers, local model gateways, build daemons, Kubernetes-in-Docker
clusters. Nothing owns them once they are started. On one host on 2026-10-16, 57 such processes
were found still running after the sessions that started them had ended, through six mechanisms:

| Mechanism | Found |
|---|---|
| A user systemd unit and timer written by a session | 1 |
| User systemd units enabled by a session and never disabled | 3 |
| Detached processes re-parented to PID 1 | 7 |
| Background shells and wait loops nobody awaited | 23 |
| A wait loop spinning because the tool it waited on was absent | 1 |
| Docker and k3d containers left running or with a restart policy | 22 |

Every one of these sessions worked under a written rule against leaving processes behind. A rule
does not fix this. What fixes it is one owner that holds a record for each process, names the
session that asked for it, ends it when its lease expires, and can list everything it holds.

Substrate already runs things and reports what it observed, holds leases and a durable operation
ledger, and owns a delegated cgroup v2 subtree in which it can kill a whole process tree. It cannot
hold these processes today. Confined exec is deliberately the wrong shape for them:
[ADR 0004](0004-the-host-driver-refuses-without-linux-confinement.md) gives exec a network namespace
with no egress and a confined filesystem, and refuses rather than fall back to running unconfined. A
development server needs the caller's repository as its working directory, the network, and a
listening port on the host.

A second failure follows from the first. With no record to ask, a session that waits for a process
waits on its side effects: it polls a log for a line in a `until grep …; sleep` loop, or waits on a
file with `inotifywait`. The line never comes, the pattern never matches, the watched tool is
missing and the loop spins, or the loop's own timeout fires and nobody checks why. A command that
takes seconds is waited on for hours. Of the 57 processes above, 24 were such wait loops. A process
that ends on a deadline of its own, and says when and why it ended through a stream the caller can
read, removes the reason to write them.

## Decision

Add a new resource family, the **host process** (a supervised process), distinct from exec and
from sessions. A host process runs in the caller's real filesystem and network, inside a cgroup of
its own under the daemon's delegated root, under a lease, with an owner and a reason, and with a
bounded log, optionally with a hard deadline, and with a stream of its events. Substrate records it, observes it, lists it, and stops it. It never confines it, and it
says so on every record.

Exec, sessions and their isolation floor are unchanged. No exec request can reach the host profile,
and no host-process request can reach the confined one.

### S1: start

`POST /v1/processes` starts one host process. The request carries:

| Field | Meaning | Validation |
|---|---|---|
| `profile` | The confinement profile; the only value is `host` | Required. Absent: `process.profile-required` (422). Any other value: `process.profile-unknown` (422) |
| `argv` | The program and its arguments, executed directly with no shell | Non-empty, bounded count and bytes, no NUL: `process.argv-invalid` (422) |
| `cwd` | The working directory | Absolute and canonical, an existing directory the daemon opens without following symlinks: `process.cwd-invalid` (422) |
| `env` | The complete environment, as a bounded name-to-value map | Bounded count and bytes, valid names: `process.env-invalid` (422) |
| `owner` | `{ "session": <opaque id>, "name": <name> }` | Both bounded and non-empty: `process.owner-invalid` (422) |
| `reason` | Why the process exists, as free text | Required and bounded: `process.reason-required` (422) |
| `lease` | `{ "ttl_ms": <n> }` or `{ "keep": true }` | See S4 |
| `timeout_ms`, `expires_at` | Optional hard deadlines | See S11 |

**Environment passthrough is the client's.** The daemon cannot see the caller's environment and does
not inherit its own into the process. The process's environment is exactly the request's `env`; a
client offers passthrough by reading its own environment and sending the names it selects.
Environment values are secrets in practice, so the durable ledger records each name and a digest of
the whole map, never a value. A start interrupted between its durable record and its dispatch is
concluded `abandoned` on restart and is never re-dispatched, because the values it needed were never
stored.

The owner `name` is unique among the subject's non-terminal records with the same owner `session`.
A second start that would duplicate it is `process.name-conflict` (409).

### S2: the `host` profile

The `host` profile runs the program as the daemon's own uid, in the host mount, network, PID, IPC
and user namespaces, with no bubblewrap, no seccomp filter, no capsule and no workspace. Every record
and every observation of a host process carries `"profile": "host"` and `"confinement": "none"`.
What Substrate does enforce for it is listed beside that: its own cgroup, whole-tree kill, the lease
and the log bound.

This keeps [invariant 3](../AGENTS.md#invariants) and the
[safety envelope](../AGENTS.md#safety-envelope) intact, because it changes nothing they govern and
claims nothing it does not do:

- **It is requested, never defaulted.** A start with no `profile` is refused. No other route, field
  or capability selects it, and a confined exec that cannot be confined still answers
  `exec.sandbox-unavailable`. There is no path by which a request for confinement ends in a host
  process.
- **It is not a degradation of anything.** The isolation floor is the floor of exec and sessions,
  and remains so. A host process does not weaken that floor; it is a different resource that
  declares the absence of isolation as a fact, the way an absent capability is reported absent.
- **It is reported, never implied.** `confinement: none` is on the record, in the listing, in the
  log header and in the operation result. A client cannot read a host process as confined.
- **It is personal.** The `host` profile is served only on the personal Unix socket, whose subject
  is `local:<uid>` from kernel peer credentials, and only when that uid equals the daemon's own
  effective uid. A peer of another uid is refused `process.host-subject-mismatch` (403): running as
  the daemon's uid on another user's behalf would hand that user the daemon's identity. A daemon
  that runs as a dedicated unprivileged identity therefore serves host processes to that identity
  alone, which in practice means to no caller.
- **It is refused in every other posture.** A request arriving over the static-bearer TCP
  listener, the hosted Identity admission profile or any network session authority is refused
  `process.host-posture-refused` (403), whatever the caller's grant says. The capability snapshot
  of such a daemon does not publish `process.host`. A hosted deployment cannot be configured to
  serve it; enabling it there is a new decision, not an option.

The capability fact `process.host` is published only when the delegated cgroup subtree is usable
for process cgroups (S3). Without it, a start is `process.host-unserved` (501) and nothing runs.

### S3: one cgroup per process

Each host process gets one cgroup created under the daemon's delegated cgroup v2 root before the
program is executed, and the program is placed in it before its first instruction (`clone3` with
`CLONE_INTO_CGROUP`), so no child it forks can start outside it. Stopping a host process signals
the cgroup, `SIGTERM` to every member, waits a bounded grace period, then writes `cgroup.kill`. A
process that daemonizes, re-parents itself to PID 1 or forks a detached child is still in that
cgroup and still dies with its record. Membership, not the process tree, defines the process.

A record's state is `starting`, `running`, `stopping` or `ended`. An ended record names exactly one
`ended_by`: `exit` (the main process exited), `stop` (an explicit stop, S5), `lease-expiry` (S4),
`timeout` or `expires-at` (S11), or `vanished`. When the main process exits and other members
remain, the record reports the main exit status, the remaining members are stopped as for an
explicit stop, and `ended_by` stays `exit`: a record ends with an empty cgroup. The exit is reported
as a code or as the terminating signal, never both. Substrate never restarts a host process.

On restart the daemon reconciles every non-terminal record against the cgroup tree. A record
whose cgroup still has members is observed again; a record whose cgroup is gone ends `vanished`,
never `running` and never with an exit status nobody observed.

### S4: leases

A lease is either a TTL within the existing `MIN_LEASE_TTL_MS`..`MAX_LEASE_TTL_MS` bounds, renewed
through `POST /v1/processes/{process_id}/lease/renew` as exec and session leases are, or `keep`. An
expired lease stops the process exactly as S3 stops it, through an operation the daemon originates
with its own actor, and the record ends with `ended_by: lease-expiry`.

`keep` has no TTL and requires its own reason, `lease.keep_reason`, distinct from the start reason.
A `keep` without one is `process.keep-reason-required` (422). A TTL outside the bounds is
`process.lease-invalid` (422). A lease may be renewed, or changed from `keep` to a TTL, but not
changed to `keep` after start: a process that outlives its sessions is decided when it is asked for.

### S5: stop by owner

`POST /v1/processes/{process_id}/stop` stops one record. `POST /v1/processes/stop` with
`{ "owner_session": <id> }` stops every non-terminal record of the authenticated subject with that
owner session, records one operation per process, and returns their ids. It is the call a caller's
session-end handler makes. Stop by owner never reaches another subject's records, and the owner
session is a label within the subject, not an identity: the subject is the only authority. Records
held under `keep` are stopped by owner too unless the request says `{ "except_keep": true }`.

### S6: listing

`GET /v1/processes` lists the subject's records, filterable by owner session, name and state, with
a bounded page. Each entry carries: id, owner session and name, reason, profile and confinement,
`argv`, `cwd`, start time, lease (TTL, renew-by and state, or `keep` and its reason), effective
deadline (S11), state, `ended_by` and the main exit status when observed, and observations: cumulative CPU from `cpu.stat`, memory from
`memory.current`, and listening ports. Memory is reported as the cgroup's `memory.current` and named
`memory_bytes`, not RSS, because it includes page cache. Listening ports are the TCP and UDP sockets
in a listening state whose inodes are held by a member of the cgroup, observed at listing time. Each
observation carries its own `observed_at`, and an observation the daemon could not make is absent,
never zero. Environment values are never listed.

### S7: logs

The program's standard output and error go to one bounded log file per record under the daemon's
owner-private state directory, never to the caller's working directory and never to a temporary
directory. Each line records its stream. At the byte bound the oldest bytes are discarded and the log
says how many were. `GET /v1/processes/{process_id}/logs` returns a bounded tail, by bytes, of either
stream or both, and states the discarded count. A log is removed with its record, and terminal records
are retained for a bounded time and count.

### S8: containers and clusters

The minimum slice of the future workloads and endpoints families is a host-process record whose
backend is a container runtime rather than a cgroup. `POST /v1/processes` with
`"backend": "docker"` and a container request, or `"backend": "k3d"` and a cluster request, starts it
through that runtime's own command, with the restart policy `no` and three labels: the record id,
the owner, and the lease's renew-by. A caller-supplied restart policy other than `no` is
`process.container-restart-refused` (422), and a caller-supplied label in Substrate's label prefix is
`process.container-label-reserved` (422). Lease expiry and stop remove the container or delete the
cluster, and the listing includes them, with the runtime's own observations of state, CPU, memory
and published ports. The capability facts `process.container.docker` and `process.container.k3d` are
published only when the runtime answers; without one, a start is `process.container-unserved` (501).
A container record still carries `profile: host` and `confinement: none`: its isolation is the
runtime's, which Substrate neither verifies nor reports as its own.

This slice is records and lifecycle only. Images, volumes, networks, endpoints and scheduling remain
the future families' decisions.

### S9: adopt

`POST /v1/processes/adoptions` imports something that is already running as an **ownerless**
record, so that a one-time sweep of survivors can list them, attach a lease and stop them. It takes
one of a pid, a user systemd unit or a container id, plus a reason. A pid is adopted with the cgroup
it is in: Substrate moves the process's whole current cgroup membership into a new cgroup of its own
under the delegated root where the kernel allows it, and refuses `process.adopt-unmovable` (409)
where it does not, rather than record a process it cannot stop whole. A unit is adopted by its
control group and is stopped through the unit's own manager. A container is adopted by id, and its
restart policy is set to `no`. A target that does not exist is `process.adopt-target-absent` (404);
one owned by another uid is `process.adopt-target-foreign` (403).

An adopted record has no owner session, `reason` as given, a lease the adoption sets, and
`"adopted": true`. It carries no `argv` it did not observe, no log for the time before adoption, and
no exit status it did not see. A later call may give it an owner.

### S11: hard deadlines

A start may carry `timeout_ms`, a wall-clock limit measured from the start, and `expires_at`, an
absolute RFC 3339 UTC instant. Either, both or neither may be given; with both, the earlier wins.
Neither is a lease: neither can be renewed, extended or removed after start, and neither is
affected by a lease renewal or by `keep`. A `keep` process with a timeout is coherent and ends at
its timeout.

At start the daemon records the effective deadline as one absolute instant in the durable record,
so a deadline survives a daemon restart and is enforced on reconciliation: a process found running
past its deadline is stopped at once. At the deadline the process is stopped as S3 stops it,
`SIGTERM`, the bounded grace, then `cgroup.kill`, through an operation the daemon originates, and the
record ends with `ended_by: timeout` or `ended_by: expires-at`. Whichever of exit, stop, lease
expiry or deadline is recorded first is the one `ended_by` names; a later one does not overwrite it.

A `timeout_ms` that is zero or above `MAX_PROCESS_TIMEOUT_MS` is `process.timeout-invalid` (422).
An `expires_at` that is not an RFC 3339 UTC instant, is not later than the daemon's clock by at least
`MIN_LEASE_TTL_MS`, or lies beyond the same maximum, is `process.expires-at-invalid` (422). The
listing shows the effective deadline and which field set it.

### S12: the per-process event stream

A host process's lifecycle transitions are events in the **existing** event log: `process.started`,
`process.lease-renewed`, `process.lease-expired`, `process.timed-out` (for `timeout` and
`expires-at`, which the observation distinguishes), `process.stopped`, `process.exited` (with the
code or the signal) and `process.vanished`, each with `resource_kind: process` and an operation
cause. They are new closed variants of the one event union, ordered by the one `seq`, paged by
`GET /v1/events` with its cursor and streamed by the SDK's `event_stream`, as every other family's
transitions are. A caller that already follows the event log needs nothing new to see a process end.

Output lines are **not** events in that log. Writing every output line into the durable ledger
would let one chatty process fill the retained history that every other consumer resumes from. They
are served only by a second, per-process stream, `GET /v1/processes/{process_id}/events`, a
WebSocket over the same socket and posture as the session attach route, and by an SDK call over it.
That stream interleaves, in one order, the process's lifecycle events from the event log and its
output lines read from the S7 log, and ends with exactly one terminal item naming `ended_by`, after
which it closes. It never ends without one: a daemon that cannot deliver it closes the stream with
an error, and a reader reconnects.

Its cursor is per process and opaque: it encodes the last delivered event `seq` and the log byte
offset. A reader resuming from a cursor receives what followed it. A reader resuming from a log
offset the log bound has already discarded receives one `gap` item stating the discarded byte
count, then continues; lines are never silently skipped. A stream opened on an ended record
delivers what the retained log and the event log still hold, the terminal item, and closes.

Output lines are bounded. A line is at most `MAX_PROCESS_EVENT_LINE_BYTES` bytes; a longer line is
delivered cut at that bound with `truncated: true` and its full length, and is never split into two
lines. Bytes that are not UTF-8 are delivered with replacement characters and `lossy: true`; the log
keeps the bytes. Each line names its stream. Because the stream reads the log file rather than the
process's pipes, a slow or absent reader never applies backpressure to the program: the S7 log bound
is the only buffer, and the `gap` item is what a reader that fell behind it sees. Concurrent readers
of one process are bounded by `MAX_PROCESS_EVENT_READERS`; beyond it, `process.events-readers-exceeded`
(429).

The stream adds no matching. Waiting for a pattern, a port or a file is a caller's watcher built on
it, outside Substrate; the stream ships before such watchers so that they read it instead of polling.

## Named refusals

| Refusal | Status |
|---|---|
| `process.host-unserved` | 501 |
| `process.host-posture-refused` | 403 |
| `process.host-subject-mismatch` | 403 |
| `process.profile-required` | 422 |
| `process.profile-unknown` | 422 |
| `process.argv-invalid` | 422 |
| `process.cwd-invalid` | 422 |
| `process.env-invalid` | 422 |
| `process.owner-invalid` | 422 |
| `process.reason-required` | 422 |
| `process.name-conflict` | 409 |
| `process.lease-invalid` | 422 |
| `process.keep-reason-required` | 422 |
| `process.limit-exceeded` | 429 |
| `process.not-found` | 404 |
| `process.container-unserved` | 501 |
| `process.container-restart-refused` | 422 |
| `process.container-label-reserved` | 422 |
| `process.adopt-target-absent` | 404 |
| `process.adopt-target-foreign` | 403 |
| `process.adopt-unmovable` | 409 |
| `process.timeout-invalid` | 422 |
| `process.expires-at-invalid` | 422 |
| `process.events-readers-exceeded` | 429 |

`process.limit-exceeded` bounds the number of non-terminal records per subject. Every bound is a
wire constant of the successor bundle.

## Consequences

**Invariant 6.** The family adds routes, schemas, capability facts and refusals, so it rides **a
successor bundle**: the next unreleased bundle after `0.18.0` that is cut for it. `0.19.0` may be
taken by a pending change to workspace addressing; this decision does not claim a version number.
No released bundle changes. The successor is additive: no existing route, field or code changes
meaning.

**Invariant 5.** Start, stop, stop by owner, lease renewal, lease expiry, deadline stop and
adoption are each an operation recorded durably before the driver acts. Lease expiry and a deadline
stop are operations the daemon originates under its own actor, so one interrupted by a crash
completes on restart.

**The event log.** The `process.*` transitions extend the closed event union; consumers that
validate the union must accept them from the successor bundle on. Output lines never enter the event
log.

**Invariant 7.** Every new request, record, observation, log-tail and refusal document has exactly
one schema in the successor bundle and validates under `cargo xtask check-json`.

**Subject.** The subject is derived from kernel peer credentials and never from the request. The
owner session is caller-supplied data scoped beneath that subject, and grants no authority.

**Invariant 4.** Clients read the record's `backend` and `profile` as declared fields of the
resource, not as which driver answered. The process driver and the container backends implement one
host-process contract and publish verified capability facts.

**Specification.** The family is specified in ESS under
`spec/host-processes/` before implementation, and each story's acceptance
is a set of named conformance scenarios there.

**What stays out.** Substrate decides no scheduling policy: which processes a caller may run, how
many beyond the record bound, at what priority, or when. It decides no product policy: whether a
session may hold a `keep` lease, which owner sessions exist, or what a reason must say. It does not
restart, health-check or supervise a process in the init-system sense; supervision here means
record, observe, bound and end. It does not confine a host process, and does not offer a profile
between `host` and exec's floor.

The client command-line tool over the socket is a separate change governed by
[ADR 0022](0022-the-rust-sdk-remains-a-wire-client.md); it adds no decision here.

## Verification

Each slice's acceptance is a named set of conformance scenarios under
`spec/host-processes/`, driven by the delegated lane where a cgroup is
required and reported absent, never passed, where it is not. The refusals of S2 are also
asserted in the portable lane, on a daemon that serves no host process.
