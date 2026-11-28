# Design 38: a long-lived service on the host driver

Status: draft, written with the default (option A) of each of the four questions of § 15. D1 to D4
are open with the operator; none is answered. Written against `main` at `1a10a30d68`. Second
revision: the claims the first revision inferred were read where the tree allows (§ 16). Serves
`epic:long-lived-service-on-the-host-driver` and its six draft stories, whose requirement is the gap
list of the `weft` repository (`cloudspec` `docs/platform-ports.md` § 10, commit `2ed72c2`). The
stakeholder of that requirement is the session that maintains `weft`.

The consumer runs a long-lived service from an image on a workstation with no container daemon.
The host driver cannot today: a background exec has no restart policy ([design 01](01-contract.md)
§ 3.2), a host process is supervised and unconfined and does not restart (ADR 0036), design 01 § 8
marks Workload unserved on the host driver, and [`STATUS.md`](../../STATUS.md) states that
workloads, images, volumes and endpoints are absent from the wire. This document decides six
things: a root filesystem from a digest-named archive (gap 5), a supervised confined workload
(gap 1), an inbound endpoint (gap 2), a local endpoint attributed to its run (gap 3), a volume kept
across runs (gap 6), a secret value given to one run (gap 4). Gap 7 is already served (§ 9).

It adds no code and no bundle. It proposes six additive successor bundles, `0.30.0` to `0.35.0`,
one per slice (§ 2); the numbers are proposals. § 14 lists the decisions, § 15 the four questions,
§ 16 what is still inferred, § 17 what is not decided, § 18 what was not read. Invariant 8 of
[`AGENTS.md`](../../AGENTS.md) applies: an ADR follows each of D1 to D4 once answered, before code
of that slice. Design 01 text this document contradicts (§ 3.3, § 3.5, § 7.7, § 8) is edited with
the first slice that makes it untrue, not here.

## 1. Evidence: what the tree does at `1a10a30d68`

Every row names the file and the item that was read. A claim not in this table is in § 16.

| what | where |
|---|---|
| design 01 specifies workloads (deploy, get, list, replace, stop, start, restart, delete, logs; spec `{name, image, command?, args?, env, secret_slots?, mounts?, endpoints?, restart, resources?, lease_ttl?, labels}`; status `{desired, observed: {state, probed_at}, restarts, last_exit}`, `observed: null` for "cannot answer"), volumes (delete refused `conflict` while mounted) and endpoints (`loopback` or `lan`; `lan` refused unless daemon configuration allows it; the answer carries the observed address); a workload with a lease is stopped on expiry | [`01-contract.md`](01-contract.md) §§ 3.3, 3.5, 5 |
| design 01 marks Workload and Image `✗` for the host driver | [`01-contract.md`](01-contract.md) § 8 |
| design 01 § 7.1: one operation is one method, path and JSON body; § 7.5: every mutation is keyed, with one explicit bounded non-keyed exception; § 7.7: "No secret values in ordinary JSON, ever" | [`01-contract.md`](01-contract.md) § 7 |
| the daemon has no bulk route: every mutation reads a body of at most `BODY_LIMIT` (2,097,152 bytes) within `REQUEST_BODY_READ_TIMEOUT` (10 s) as an `{op, input}` JSON envelope | `BODY_LIMIT`, `REQUEST_BODY_READ_TIMEOUT` in [`app.rs`](../../crates/substrate-daemon/src/app.rs); `read_bounded_body`, `decode_mutation` in [`operations.rs`](../../crates/substrate-daemon/src/app/operations.rs) |
| bytes ride JSON as base64: a file is at most `MAX_FILE_BYTES` (1,048,576), its base64 form `MAX_FILE_CONTENT_BASE64_CHARS` (1,398,104) | [`substrate-wire/src/lib.rs`](../../crates/substrate-wire/src/lib.rs) |
| the request hash is an unkeyed SHA-256 over the method, the address, the raw input JSON and the query; the ledger row (`operations`) stores that hash, the state, the actor, the resource and the outcome JSON, and has no input column; an exec's record is `execs.resource_json`; a replay compares the hash and a different one is `operation.request-conflict` (409) | `canonical_request_hash_v2` in [`substrate-wire/src/lib.rs`](../../crates/substrate-wire/src/lib.rs); `operations`, `execs` in [`schema.rs`](../../crates/substrate-store/src/schema.rs); `decode_mutation`, `reservation_response` in [`operations.rs`](../../crates/substrate-daemon/src/app/operations.rs) |
| the ledger holds at most 100,000 rows and 512 MiB per subject, 1,000,000 rows and 4 GiB per deployment | `OPERATION_LEDGER_*_MAX_*` in [`substrate-wire/src/lib.rs`](../../crates/substrate-wire/src/lib.rs) |
| error classes are `refused`, `conflict`, `unserved`, `exhausted`, `failed`; a conflict answers 409; a not-found is class `refused` with 404 | `ErrorClass` in [`substrate-wire/src/lib.rs`](../../crates/substrate-wire/src/lib.rs); `conflict` in [`responses.rs`](../../crates/substrate-daemon/src/app/responses.rs); `DriverErrorClass::NotFound` in [`operations.rs`](../../crates/substrate-daemon/src/app/operations.rs) |
| an exec runs under bubblewrap with `--unshare-user --disable-userns`, `--unshare-ipc --unshare-pid --unshare-net --unshare-uts`, `--new-session`, `--die-with-parent`, `--clearenv`, a seccomp program; the child is `kill_on_drop(true)`; no `--uid` is passed anywhere in the host crate | `command`, `USER_NAMESPACE_ARGV` in [`process.rs`](../../crates/substrate-host/src/process.rs) |
| the base view is `--ro-bind /usr`, `--ro-bind-try` of `/bin`, `/lib`, `/lib64`, `/etc/alternatives`, `--proc`, `--dev`, `--tmpfs /tmp`; the rest of the root is bubblewrap's tmpfs; without the exec-gate helper the program runs as `/usr/bin/env -u PWD -- <argv>` | `command` in [`process.rs`](../../crates/substrate-host/src/process.rs); [ADR 0049](../../adr/0049-the-base-view-carries-etc-alternatives-and-tmp-takes-an-operator-size.md) |
| roots are bound by descriptor (`--ro-bind-fd`, `--bind-fd`), never by `/proc/self/fd/<n>` as a path | [`read_only_roots.rs`](../../crates/substrate-host/src/read_only_roots.rs), [`writable_roots.rs`](../../crates/substrate-host/src/writable_roots.rs), `bind_args` in [`process.rs`](../../crates/substrate-host/src/process.rs); ADRs [0010](../../adr/0010-declared-host-roots-are-mounted-read-only.md), [0038](../../adr/0038-writable-roots-are-named-host-paths-or-git-checkouts.md) |
| every exec has its own cgroup v2 directory created before the spawn, with `pids.max`, `memory.max`, `memory.swap.max` 0 and a `cpu.max` quota; `ExecLimits` is `{timeout_ms, output_bytes, processes, memory_bytes, cpu_millis}` where `cpu_millis` is a cumulative budget, and the quota is derived from it over `timeout_ms`, capped at `exec_cpu_cores` periods | `Cgroup::create`, `exec_cpu_quota` in [`process.rs`](../../crates/substrate-host/src/process.rs); `ExecLimits` in [`substrate-wire/src/lib.rs`](../../crates/substrate-wire/src/lib.rs); `exec_cpu_cores` in [`lib.rs`](../../crates/substrate-host/src/lib.rs) |
| the egress aperture is a helper that joins the run's namespace once (`NS_GET_USERNS`, `setns`), binds, listens and hands the listener out over `SCM_RIGHTS`; a forwarder that never leaves the host namespace; a relay per connection; the helper cannot also be the forwarder; the child pid comes from `--info-fd`; apertures listen on `127.0.0.1` to `127.0.0.8`, at most `MAX_EGRESS_APERTURES` (8) per run and `MAX_DEPLOYMENT_EGRESS_APERTURES` (12) per deployment | module documentation of [`egress.rs`](../../crates/substrate-host/src/egress.rs); [ADR 0013](../../adr/0013-egress-apertures-are-declared-by-the-operator.md) and its amendments |
| an applied aperture is `{mode: "aperture", name, destination (3 to 64 characters), mechanism (the one value "loopback-forwarder"), bytes: {to_destination, from_destination}, max_bytes?, listen?}`; the array form (2 to 8 elements) requires `listen` matching `^127\.0\.0\.[1-8]:[0-9]{1,5}$`; the single form has no `listen`; the capability fact `exec.egress-apertures` lists `{name, destination, max_bytes?}`, at most 12 | `AppliedAperture`, `AppliedNetwork`, `EgressApertureFact` in [`substrate-wire/src/lib.rs`](../../crates/substrate-wire/src/lib.rs); `applied-network` in [`schemas/resource.json`](../../contracts/substrate-wire/0.29.0/schemas/resource.json), `exec.egress-apertures` in [`schemas/capability.json`](../../contracts/substrate-wire/0.29.0/schemas/capability.json) |
| exec, session and process ids are at most 32 characters | `exec-id`, `session-id`, `process-id` in [`schemas/common.json`](../../contracts/substrate-wire/0.29.0/schemas/common.json) |
| host processes: own cgroup, bounded log ring (`MAX_PROCESS_LOG_BYTES` 8 MiB, line 16 KiB, read 1 MiB), event stream, leases, reconciliation; at most `MAX_PROCESSES_PER_SUBJECT` (64); a process's environment values are never stored, so a start interrupted before dispatch is concluded `abandoned` and never re-dispatched; the host-process supervisor "does not restart" a process | [ADR 0036](../../adr/0036-host-processes-are-a-supervised-resource-family.md); [`host_process/`](../../crates/substrate-host/src/host_process/); `HostProcessLimits` in [`host_processes.rs`](../../crates/substrate-store/src/host_processes.rs) |
| a lease expiry and a deadline stop are operations the daemon originates under its own actor (`op_supervisor_<ulid>`), recorded durably before the driver acts; the daemon tracks a start between its record and its dispatch (`DriverRun`) | `supervisor_operation`, `DriverRun`, module documentation of [`app/processes.rs`](../../crates/substrate-daemon/src/app/processes.rs) |
| the event union is closed: each variant has a `resource_kind`, a resource id, a `transition` from a fixed list, a `cause` of kind `operation` with an operation id, and an observation | [`schemas/event.json`](../../contracts/substrate-wire/0.29.0/schemas/event.json); `PROCESS_EVENT_TRANSITIONS` in [`substrate-wire/src/lib.rs`](../../crates/substrate-wire/src/lib.rs) |
| a secret slot is `{slot, fd}` in the request ("a name and a number each; never a value, a path or a length") and the applied record echoes the same shape; it is a sealed memfd, at most `MAX_SECRET_SLOTS` (8) | `SecretSlotRequest`, `ExecStartInput`, the applied confinement in [`substrate-wire/src/lib.rs`](../../crates/substrate-wire/src/lib.rs); [`memfd.rs`](../../crates/substrate-host/src/memfd.rs), [`secrets.rs`](../../crates/substrate-host/src/secrets.rs); [ADR 0012](../../adr/0012-secret-slots-are-sealed-memfds.md) |
| errors and events never carry secret material; a slot name is allowed in an error | [`04-security-and-isolation.md`](04-security-and-isolation.md) § 6; `valid_secret_slot_name` in [`substrate-wire/src/lib.rs`](../../crates/substrate-wire/src/lib.rs) |
| the exec-gate helper is bound from sealed bytes at `GATE_MOUNT` (`/.substrate-exec-gate`) with `--ro-bind-data` and is the first program after bubblewrap; the driver judges an executable-list entry against `BASE_MOUNTS` (`/usr`, `/bin`, `/lib`, `/lib64`), "bound from the host's own at the same paths"; design 35 D3 makes an absolute entry eligible only beneath those, `/workspace` and declared mounts | `GATE_MOUNT`, `BASE_MOUNTS` in [`exec_gate.rs`](../../crates/substrate-host/src/exec_gate.rs); [`35-executable-lists.md`](35-executable-lists.md) D3, D5, D20 |
| project quotas bound writable storage | [`quota.rs`](../../crates/substrate-host/src/quota.rs); [ADR 0020](../../adr/0020-writable-storage-uses-delegated-project-quotas.md); design 01 § 10, decision 6 |
| `rename_noreplace` (`renameat2` with `RENAME_NOREPLACE`) exists; `openat2` with `RESOLVE_BENEATH`, `RESOLVE_NO_SYMLINKS`, `RESOLVE_NO_MAGICLINKS`, `RESOLVE_NO_XDEV` is the guarded file layer; `linkat` is used nowhere | [`rename.rs`](../../crates/substrate-host/src/rename.rs); [`substrate-guarded/src/fs.rs`](../../crates/substrate-guarded/src/fs.rs) |
| the store adds tables with `CREATE TABLE IF NOT EXISTS` and alters one only through a named `migrate_*` function; it has no table for workloads, images, volumes or endpoints | [`schema.rs`](../../crates/substrate-store/src/schema.rs) |
| the daemon names the host crate only in the composition root | [`driver_port.rs`](../../crates/substrate-daemon/tests/driver_port.rs); invariant 4 |
| the lock file has no tar or inflate crate; it has `sha2`, `zeroize`, `getrandom`, no `hmac`; the registry holds `flate2` 1.1.11 (`MIT OR Apache-2.0`), `miniz_oxide` 0.8.9 (`MIT OR Zlib OR Apache-2.0`, `#![forbid(unsafe_code)]`), `tar` 0.4.46 (`MIT OR Apache-2.0`); the gate accepts exactly the licences of `about.toml`, which include those | [`Cargo.lock`](../../Cargo.lock); [`about.toml`](../../about.toml); the crates' `Cargo.toml` in the local registry |
| no released bundle holds a route for workloads, images, volumes or endpoints, and none holds the word `lan` | [`operations.json`](../../contracts/substrate-wire/0.29.0/operations.json) and a search of `contracts/substrate-wire/0.29.0` |
| precedent for a successor bundle that adds optional start members, a fact, applied members and a refusal table with a vector each | [`37-executable-lists-on-the-wire.md`](37-executable-lists-on-the-wire.md) §§ 6, 9 |

Three things the table decides. The host driver has the pieces (confinement, cgroups, descriptor-bound
mounts, a supervisor, a log ring, a relay across the namespace boundary) and not the family: no
resource holds a restart policy, a root other than the host's `/usr`, or a way in. Exec children die
with the daemon and host processes do not (`kill_on_drop(true)` against `false`), so a workload on exec
confinement inherits the first (§ 4.6). The wire carries no bulk bytes and no secret value, which
shapes § 3.2 and § 8.

## 2. Scope and order of slices

The order is the consumer's: gaps 5, 1, 2, 3 first; gap 6 after gap 1 as the epic orders; gap 4 last
because D4 comes after the others.

| slice | proposed bundle | gap | adds | needs |
|---|---|---|---|---|
| 1 | `0.30.0` | 5 | images: create, chunked upload, complete, get, list, delete; `image` on exec and session start | D2 |
| 2 | `0.31.0` | 1 | workloads: deploy, get, list, stop, start, restart, delete, logs | D1, slice 1 |
| 3 | `0.32.0` | 2 | endpoints; `endpoints` on the workload spec | D3, slice 2 |
| 4 | `0.30.0` (landed first, ADR 0054) | 3 | a local endpoint, selected by name in `aperture`/`apertures` | execs: nothing; workloads: slice 2 |
| 5 | `0.34.0` | 6 | volumes; `mounts` on exec, session and workload | slice 2 |
| 6 | `0.35.0` | 4 | `secret_values` on exec, session and workload | D4, slice 2 |

Slices 3 and 4 touch execs first and workloads when slice 2 is in. A slice takes the next bundle number
when it lands, so the numbers of this table move: slice 4 landed first and published `0.30.0`, and the
others take the numbers after it. A slice never rewrites a released
bundle (invariant 6), states its predecessor and route counts, and is held by
`cargo xtask check-bundle <version>` and a line of `scripts/gate.sh`.

## 3. Root filesystems (gap 5, D2)

The daemon unpacks a gzip tar named by its sha256, in Rust, into a content-addressed store it owns;
a run mounts that tree read-only as its `/`.

### 3.1 The resource

An image is identified by `sha256:<64 lower-case hex>` of its archive bytes (design 01 § 1: images
go by digest). A record states `digest` (the observed digest), `size_bytes`, `entries`,
`unpacked_bytes`, `state` (`receiving`, `available`) and `created_at`. The record is per subject; the
unpacked tree is per digest and shared (§ 3.4). A record of another subject is the same
`image.not-found` as an absent one (design 04 § 9).

### 3.2 The routes: chunked, inside the existing envelope

The daemon has no bulk route (§ 1). A raw-body `PUT` would be a new route class that breaks design 01
§ 7.1 (one operation is one method, path and JSON body), which that section calls a breaking change.
The first slice therefore carries the archive in keyed `{op, input}` operations of at most the size
a file already travels in, and adds no new body handling.

| route | operation | notes |
|---|---|---|
| `POST /v1/images` | `images.create` | `{digest, size_bytes}`; answers `receiving`; durable before any byte is received (ADR 0005) |
| `POST /v1/images/{digest}/chunks` | `images.chunk` | `{offset, content}`, `content` base64 of at most `MAX_IMAGE_CHUNK_BYTES` (1,048,576, `MAX_FILE_BYTES`); `offset` must equal the bytes received, else `image.chunk-out-of-order` (409); answers `{received_bytes}` |
| `POST /v1/images/{digest}/complete` | `images.complete` | verifies size and digest, unpacks, answers the record `available` |
| `GET /v1/images[/{digest}]` | read | |
| `DELETE /v1/images/{digest}` | `images.delete` | `image.in-use` (409) while held (§ 3.4) |

A 100 MiB archive is 100 chunk operations and about as many ledger rows, against the subject's bound of
100,000 rows (§ 1). A retried chunk replays by operation id. A raw-body upload is faster and is
§ 17: it needs an exception to design 01 § 7.1, the way § 7.5 records one for snapshots.

### 3.3 What the daemon does with the bytes

1. Chunks append to a file in an owner-private staging directory while the daemon computes the
   sha256. `complete` with a received size other than `size_bytes` is `image.size-mismatch` (422).
2. **The digest is compared before anything is parsed.** A mismatch is `image.digest-mismatch` (422),
   stating the observed digest; the staging file and the record are removed; nothing else exists.
3. The daemon inflates and reads the tar from the staging file and creates each entry in a second
   staging directory under the rules below.
4. It publishes the finished tree (§ 3.4) and marks the record `available`.

**Rules of the unpack.** Each is `image.entry-refused` (422) unless another code is stated; a refusal
removes the staging directory.

- Kinds: regular file, directory, symbolic link, hard link. Device, FIFO, socket, sparse and any
  other kind are refused.
- Paths are relative; a leading `./` is dropped. Absolute, empty, NUL, a `..` component, a component
  over 255 bytes, a path over `MAX_IMAGE_PATH_BYTES` (4,095) and a path named twice are refused.
- Every entry is created with `openat2` beneath the staging root, no symlink and no magic link
  followed, through `substrate-guarded`'s layer (§ 1). An entry beneath a symbolic link already
  created is refused. **A symbolic link is stored and never followed**; its target text (at most
  `MAX_IMAGE_LINK_TARGET_BYTES`, 4,095) is not interpreted. An absolute or escaping target is an
  ordinary stored link: in the run it resolves in the run's own root.
- A hard link is made only to a regular file the same archive already unpacked, with `linkat` on
  descriptors; any other target is refused.
- Modes keep the permission bits with `setuid` and `setgid` dropped and the sticky bit kept; write
  bits are cleared on the finished tree. Owners and times are not kept: every entry is the daemon's
  uid. Extended attributes, file capabilities and ACLs in pax records are ignored.
- Bounds: archive bytes `MAX_IMAGE_ARCHIVE_BYTES` (104,857,600, design 01 § 10's ceiling), unpacked
  bytes `MAX_IMAGE_UNPACKED_BYTES` (1,073,741,824), entries `MAX_IMAGE_ENTRIES` (65,536); beyond one,
  `image.archive-limit` (429), counted while inflating, so a small archive that inflates past the
  bound stops at it. The deployment may lower each; the fact states the effective value.
- Pax `path`, `linkpath`, `size` and the GNU long-name and long-link entries are honoured; other
  extensions are ignored. A malformed header, a short read, a bad checksum, or a gzip stream that does
  not end with the tar is `image.archive-invalid` (422).

**What reads the archive.** The unpacker is Rust in the host crate, in the daemon process. The lock
has no tar or inflate crate, so the slice adds dependencies. The design picks a tar reader written in
this repository, so every file is created through the descriptor layer and not a crate's own path
handling, and `flate2` with its pure-Rust `miniz_oxide` backend for the inflate. Their licences,
read from the registry copies, are inside `about.toml`'s accepted set (§ 1). Advisories were not
checked: `cargo xtask check-advisories` clones the RustSec database over the network and was not run.
Parsing untrusted bytes in the daemon is a risk this document names and does not remove (§ 17).

### 3.4 The store

- `--image-store <directory>` names it. No default, no environment variable (design 37's
  `--exec-gate` precedent). Without it the fact is absent and `images.create` is
  `image.archive-unserved` (501) before any byte.
- The directory is opened at startup and must be owned by the daemon's uid with no group or other
  permission; otherwise the fact is absent with a diagnostic.
- Layout: `staging/<random>`, `trees/<sha256>/`, `trash/<random>`. A finished tree is published by
  one `rename_noreplace`; a second create of an existing digest drops its own staging. Startup
  removes bounded, well-formed `staging/` and `trash/` entries and fails closed on anything else
  (design 04 § 3).
- New tables `images` and `image_holds` (one row per exec, session or workload that uses a tree).
  No existing table changes (§ 1, `schema.rs`).
- `DELETE` of an image held by a run, or named by a workload record, is `image.in-use` (409).
  Otherwise the record goes, and the tree when the last record of its digest goes: renamed into
  `trash/`, removed by a bounded walk that follows no link.
- A `receiving` record older than `IMAGE_RECEIVE_TTL_MS` (900,000) is removed with its staging
  file; a subject holds at most `MAX_RECEIVING_IMAGES_PER_SUBJECT` (4), `image.limit` (429) beyond.
  The store holds at most `MAX_IMAGES` (64) trees and `--image-store-bytes` of unpacked data when
  given; beyond either, `image.store-full` (429).

### 3.5 The run's view

An exec or session start gains an optional `image: {digest}`; a workload names one (§ 4). The rest
of `command` in `process.rs` is unchanged.

**What changes in the base view.** The host's `/usr`, `/bin`, `/lib`, `/lib64` and `/etc/alternatives`
are not bound. For each top-level entry of the unpacked tree the driver binds it read-only at
`/<name>` with `--ro-bind-fd` on a descriptor pinned at admission, onto bubblewrap's own tmpfs root:
bubblewrap creates the mount points, which the tree itself, being read-only, could not. `/proc`,
`/dev`, `/tmp` and the other fixed binds come after, so an image entry of that name is shadowed, not
an error.

- A mount that targets a path **beneath** an image entry (an aperture's `/etc/hosts` and CA bundle from
  `GeneratedResolution`; a later volume at `/data`) needs that path to exist in the image, because the
  image is read-only; else `image.mount-target-absent` (422) names the path. A target at a top-level
  path the image lacks needs nothing.
- The program stage `/usr/bin/env -u PWD --` is a host binary of a path the view no longer holds. An
  image run starts the program with no `env` stage; `PWD` is bubblewrap's. A start whose `env.set`
  carries `PWD` is refused `image.env-pwd-unserved` (422).
- **Executable lists with an image stay refused**, `image.exec-lists-unserved` (501). The reason, read
  in design 35 and `exec_gate.rs`: the helper's placement is not the obstacle (`GATE_MOUNT` is a file
  bubblewrap binds from sealed bytes onto its own tmpfs root, and with a list the helper is the
  first program, so no `env` is needed in the view). The obstacle is the judgement. Design 35 D3
  makes an absolute entry eligible only beneath `/usr`, `/bin`, `/lib`, `/lib64`, `/workspace` and
  declared mounts, and the driver judges those against `BASE_MOUNTS`, "bound from the host's own".
  With an image the view's `/usr` is the image's: the driver would judge the host's place and answer
  for the wrong file system, a silent wrong answer. Serving lists needs D3, D24 and D30 amended for
  image binds and a lane case for each; § 17.
- An exec's `workdir` is as today. A workload's working directory is `/` unless it names an absolute
  path (§ 4.1). A missing program or directory ends the run as it does today: an exit, not a
  refusal.

The floor, item by item, is § 10.

### 3.6 The wire of slice 1 (`0.30.0`)

- Facts: `image.archive`, `{max_archive_bytes, max_unpacked_bytes, max_entries}`, published only after
  the open proved the store directory, `openat2` beneath, and a throwaway image-view run (§ 4.9);
  absent otherwise.
- Start fields: optional `image: {digest}` on the exec and session start. Applied record:
  `image: {digest}`, the observed digest the run mounted. A start without it serializes and hashes as
  on `0.29.0`.
- Events: `image.created`, `image.deleted` (§ 4.7 states the union change).
- Refusals:

| code | status | when |
|---|---|---|
| `image.digest-invalid` | 422 | not `sha256:` and 64 lower-case hex |
| `image.archive-unserved` | 501 | the fact is absent |
| `image.chunk-out-of-order` | 409 | § 3.2 |
| `image.size-mismatch`, `image.digest-mismatch` | 422 | § 3.3; the digest refusal states the observed digest |
| `image.archive-invalid`, `image.entry-refused` | 422 | § 3.3 |
| `image.archive-limit`, `image.limit`, `image.store-full` | 429 | §§ 3.3, 3.4 |
| `image.not-found` | 404 | not a record of the subject |
| `image.in-use` | 409 | delete while held |
| `image.mount-target-absent`, `image.env-pwd-unserved` | 422 | § 3.5 |
| `image.exec-lists-unserved` | 501 | § 3.5 |

- Acceptance, from `story:root-filesystem-from-a-digest-named-archive` (weft test
  `ports-node-images-local`):
  - create an image from a gzip tar with its correct sha256: the record states that digest as
    observed, and an exec started on it reads `/etc/os-release` from the archive, not the host's;
  - the same bytes under another digest are refused with a named refusal that states the observed
    digest, nothing is left in the store, and a workload naming that image never starts;
  - an archive holding a `../` path, an absolute path, a device node, or a link pointing outside its
    root is refused by name (for a link, stored and never followed) and no file outside the store root
    is written, checked on the host;
  - a setuid binary in the archive is stored without the setuid bit;
  - delete of an image used by a running workload is refused `conflict`; after the run ends it
    succeeds and the tree is gone;
  - portable lane: without the mechanism the fact is absent and a create is refused by name.

OCI images, build, pull and push stay unserved on the host driver (design 01 § 10, decision 4).

## 4. Workloads (gap 1, D1)

A workload is a long-lived, confined, daemon-supervised run of one program from one image under a
restart policy: a resource family of the host driver beside exec and host process. ADR 0036 says the
host-process supervisor "does not restart" a process; restart lives here, in a family of its own.

### 4.1 The resource

Design 01 § 3.3 gives the spec and status. The first slice's spec:

```
{ name, image: {digest}, argv, cwd?, env, restart, limits, lease_ttl?, labels }
```

- `name`: unique among the subject's workloads; `workload.name-taken` (409).
- `argv`: the program and arguments, one list as every start takes it (design 04 § 3). Design 01's
  `command` and `args` become it; the archive carries no default command.
- `cwd`: an absolute view path, default `/`.
- `env`: the exec's closed environment shape. **A workload's `env` is stored in the `workloads` table**
  because a relaunch after a daemon restart needs it (§ 4.6); ADR 0036 stores no host-process value
  for the opposite reason. The contract declares `env` non-secret (design 04 § 3: a closed non-secret
  baseline plus admitted values); a secret travels as a slot or a `secret_values` member, never in
  `env`. The ledger holds no input in either case (§ 1).
- `restart`: `never`, `on-failure`, `always`.
- `limits`: `{processes, memory_bytes, cpu_millicores}`, all required. `cpu_millicores` is a **rate**
  (1,000 is one core), capped by the operator's `exec_cpu_cores`, and sets `cpu.max`. `ExecLimits`
  cannot be used as is: its `cpu_millis` is a cumulative budget whose quota is derived over a
  `timeout_ms` (§ 1), and a workload has neither a timeout nor a budget (W9).
- `lease_ttl`: optional, `MIN_LEASE_TTL_MS` to `MAX_LEASE_TTL_MS`; `labels` as everywhere.
- Not in the first slice: `mounts` (slice 5), `endpoints` (slice 3), `secret_slots`, `secret_values`
  (slice 6).

The id is `wl_...`. The applied record carries what an exec's does (design 01 § 9: asked and applied).
Status is design 01's:

```
{ desired: running|stopped, observed: {state, probed_at} | null, restarts,
  last_exit: {code?, signal?, at, cause} | null }
```

`cause` is `exit`, `signal`, `limit`, `stop`, `lease-expired`, `daemon-restart` or `start-failed`.

### 4.2 Operations served

| operation | route | first slice |
|---|---|---|
| deploy | `POST /v1/workloads` | yes (`workloads.deploy`) |
| get, list | `GET /v1/workloads[/{id}]` | yes |
| stop, start, restart | `POST /v1/workloads/{id}/(stop\|start\|restart)` | yes |
| delete | `DELETE /v1/workloads/{id}` | yes |
| logs | `GET /v1/workloads/{id}/logs` | yes, a bounded read of the ring; no WebSocket stream |
| lease renewal | `POST /v1/workloads/{id}/lease/renew` | yes (design 01 § 5) |
| replace | `PUT /v1/workloads/{id}` | **no**: the consumer stops, deletes and deploys; `workload.replace` is not published |

### 4.3 The state machine

Desired state is what the caller asked and is durable; observed state is what the supervisor saw.

| observed `state` | meaning |
|---|---|
| `starting` | desired `running`; cgroup and sandbox being made |
| `running` | the program's tree is in its cgroup |
| `backoff` | the program ended, the policy restarts it, the delay has not passed |
| `stopping` | a stop signalled the cgroup and waits for emptiness |
| `stopped` | desired `stopped`; the cgroup was observed empty and removed |
| `exited` | the program ended and the policy did not restart it |
| `failed` | the sandbox could not be made, or the restart limit was reached |

- **Deploy** writes desired `running` durably, then `starting`, then `running` when the spawn is proved.
- **An end of the tree.** `never`: `exited`. `on-failure`: status 0 is `exited`; any other status, a
  signal, or an end at `memory.max` or `pids.max` is `backoff`; a signal the daemon sent for a stop is
  not a failure. `always`: `backoff` unless desired is `stopped`.
- **Back-off** is `min(1,000 ms × 2^k, 60,000 ms)` with `k` the consecutive ends since a run that
  lasted 30,000 ms; after 100 consecutive ends without such a run the state is `failed`. `start`
  resets it. The constants are `WORKLOAD_BACKOFF_BASE_MS`, `_MAX_MS`, `WORKLOAD_STABLE_MS` and
  `MAX_WORKLOAD_CONSECUTIVE_RESTARTS`; a unit case on a test clock holds each. `restarts` counts every
  start after the first.
- **Stop**: desired `stopped`; `stopping`; signal the cgroup, wait the grace interval, `cgroup.kill`,
  observe emptiness (design 04 § 7); `stopped`. From `backoff`, `failed` or `exited` it goes straight to
  `stopped`; stopping a stopped workload answers the record.
- **Start** from `stopped`, `exited` or `failed`: desired `running`, `starting`; from any other state, the
  record unchanged. **Restart** is stop then start under one operation; a failed stop does not start.
- **Delete** is allowed when the state is `stopped`, `exited` or `failed` and no cgroup of the workload
  exists, else `workload.not-stopped` (409). The caller stops first; refusing is the reversible choice
  (W8). It removes the record, the ring and the `image_holds` row.

### 4.4 Confinement: exactly an exec's

A run is built by the same `command` with the image view (§ 3.5), in its own cgroup (named from the
workload id and a run counter) with `pids.max`, `memory.max`, `memory.swap.max` 0 and `cpu.max`, the
seccomp program, the non-nestable user namespace, the cleared environment and a network namespace with
loopback only. The supervisor is a module of the host crate beside `host_process`; the daemon reaches
it through the `Driver` trait only (invariant 4, `driver_port.rs`).

### 4.5 Durability and operations

ADR 0005 holds: deploy, stop, start, restart and delete are keyed operations durable before dispatch;
a new `workloads` table holds the spec, desired state, counters, last exit and last observed state.
**Which supervisor actions are ledger operations** follows the host-process precedent (§ 1,
`supervisor_operation`):

- A lease expiry is a stop the daemon originates under its own actor, recorded durably before the driver
  acts, as for a host process.
- **A restart by policy and a relaunch after a daemon restart are not operations.** A crash loop would
  write a row per restart against a subject bound of 100,000 rows, and a stable program that fails every
  31 seconds would use that in a few weeks; rows would then refuse every other mutation of the subject.
  They are events and `restarts`.
- Every event needs an operation cause (§ 4.7). The cause of a policy restart or a relaunch is the
  operation that last put the workload at desired `running`: the deploy, start or restart.

### 4.6 After a daemon restart

An exec's child dies with the daemon, and a workload on that confinement does too. **A workload does not
survive its daemon; a daemon restart relaunches it.**

Reconciliation, at startup before the workload routes answer, for each record with desired `running`:

1. `observed` is `null` and `probed_at` cleared: nothing is known.
2. A cgroup of the workload that exists is killed with `cgroup.kill`, waited empty, removed. **No
   relaunch happens before that removal.** A run's cgroup is created before its spawn (§ 1), so the
   absence of every cgroup of the workload proves no process of it, and no relaunch duplicates one. If
   removal fails the state stays `null` and the supervisor retries with the back-off; it never spawns
   while a cgroup of the workload exists.
3. Then `starting`, the run counter advances, the previous `last_exit` is `cause: daemon-restart`,
   and `restarts` counts it. The restart policy is not applied: `never` relaunches too, because
   desired is `running` and the policy governs the program's own exits (W6).

Desired `stopped`: left alone; a leftover cgroup is removed. A deploy that is durable but was never
dispatched (the `DriverRun` case: neither held nor in flight) is started by the same pass; unlike a
host process it is not `abandoned`, because its spec is stored. A relaunch re-checks the predicates of
the spec (`workload.supervised`, `image.archive`, an aperture, a volume) against the current snapshot; one
that no longer holds is `failed`, `cause: start-failed`, with the refusal's code in the event.

### 4.7 Leases and events

- A workload with a `lease_ttl` is renewed by `POST .../lease/renew`. Expiry is a typed transition
  (design 01 § 5): desired becomes `stopped`, the stop runs, and the event reason is `lease-expired`;
  the cgroup is observed empty and the record stays.
- **The event union grows a variant per transition**, as ADR 0036 S12 did for `process`: closed
  `resource_kind: workload`, `resource` a new `workload-id` (`^wl_[A-Za-z0-9]+$`, at most 32), the
  fields every variant has (§ 1) and an observation `workload-transition` of state, `restarts` and
  `last_exit`. Transitions: `workload.deployed`, `.running`, `.backoff`, `.exited`, `.failed`,
  `.stopping`, `.stopped`, `.lease-expired`, `.deleted`. None carries `env` or a log byte. The bundle
  holds the schemas (`event.json`, `common.json`, `resource.json`).
- Logs: stdout and stderr of every run go to one ring of `MAX_WORKLOAD_LOG_BYTES` (8 MiB), lines cut
  at 16 KiB, a read at most 1 MiB. It survives program restarts and ends with the record. It is the
  mechanism of `host_process/log.rs`; sharing or copying it is the story's.

### 4.8 Refusals and their order

On a deploy, each before dispatch and recorded as the daemon records every refusal of a start:

1. `request.schema-invalid` (422): shape, bounds, a `timeout_ms` or `cpu_millis` member.
2. `workload.unserved` (501): the fact is absent. **This is the portable lane's answer.**
3. `workload.limit` (429): `MAX_WORKLOADS_PER_SUBJECT` (32) or `MAX_WORKLOADS_PER_DEPLOYMENT` (128).
4. `workload.cpu-over-ceiling` (422): `cpu_millicores` above `exec_cpu_cores`.
5. `workload.name-taken` (409).
6. `image.not-found` (404) or `image.archive-unserved` (501).
7. Members of later slices, in their slice's order: `exec.aperture-undeclared`, `volume.not-found`, the
   secret refusals.
8. From the driver at dispatch: `image.mount-target-absent`, `image.env-pwd-unserved`.

A stop, start, restart or delete of an absent workload or another subject's is `workload.not-found`
(404); delete's own refusal is `workload.not-stopped` (409).

### 4.9 The wire of slice 2 (`0.31.0`)

- Routes: § 4.2. Fact: `workload.supervised`, `const true`, published only after the exec
  confinement facts were proved and a throwaway image-view run succeeded, never after reading
  configuration; absent on the portable lane and the namespace driver.
- Start fields: § 4.1. Applied record: an exec's plus `image` and the run counter.
- Refusals: § 4.8, each a wire constant with a vector; `the_refusal_codes_are_the_wire_constants`
  (`AGENTS.md` § The gate) counts them.
- Acceptance, from `story:host-driver-supervised-workload` (weft test `ports-node-workloads-local`):
  - deploy from an unpacked root filesystem with `restart: never`, a command that runs until stopped:
    observe answers `observed.state: running` within 5 s; stop; observe answers `stopped`, and no process
    of its cgroup remains;
  - `restart: on-failure`, a command that exits 1 after 1 s: observe shows `restarts` of at least 1 and
    `last_exit.code: 1`; a command that exits 0 is not restarted and ends `exited`;
  - `restart: always`: a command that exits 0 is restarted; stop ends it and it stays stopped;
  - a daemon restart while a workload runs: observe answers the same workload, `running` or
    `observed: null` until probed, never a duplicate process;
  - a workload whose lease expires is stopped and its cgroup is empty;
  - the workload process cannot open an outbound connection to an undeclared destination, and the
    isolation cases of the delegated lane hold against it;
  - portable lane: without the mechanism the workload fact is absent and a deploy is refused by name
    before dispatch.

## 5. Inbound endpoints (gap 2, D3)

An endpoint exposes a port of a confined exec or workload at an address on the host. The mechanism
mirrors the egress aperture and leaves the run with no interface.

### 5.1 The resource

`POST /v1/endpoints` (`endpoints.expose`) with `{target: {workload} | {exec}, port, exposure:
loopback|node}`; `GET`; `DELETE /v1/endpoints/{id}` (`ep_...`). The answer carries the **observed** address
`{host, port}`. A workload's spec may carry `endpoints: [{port, exposure}]`, the same endpoint made at
deploy, and `GET /v1/workloads/{id}` lists the observed addresses.

**The exposure word is `node`**, the stakeholder's, where design 01 § 3.5 says `lan`. This design renames
it; no released bundle holds either word or an endpoint route (§ 1), so nothing breaks, and design 01 is
edited with this slice (W17).

### 5.2 The mechanism

Egress has a listener inside the run's namespace drained by a forwarder in the host's (§ 1). Inbound is
its mirror, with the same constraint, that a helper inside the namespace cannot leave it:

| role | namespace | does |
|---|---|---|
| listener | host | the daemon binds the observed address: `127.0.0.1` for `loopback`, the configured node address for `node` |
| helper, one per accepted connection | joins the run's network namespace once | `setns`, one `connect` to `127.0.0.1:<port>` inside it, hands the connected socket back over `SCM_RIGHTS`, exits |
| relay, one per connection | host | copies bytes between the accepted and the returned socket |

The helper gets the namespace as the aperture helper does: the child pid from the run's `--info-fd`, the
owning user namespace through `NS_GET_USERNS`, and it waits for loopback (the three traps in
`egress.rs`), in the same forked-without-exec regime. A connected socket made in the namespace stays in
it when passed out; **the child gets no interface**, so `exec.no-egress` stays `true` for a run that
serves an endpoint, which is the answer to the story's coexistence question. The helper's only
destination is `127.0.0.1` and the declared port; no request supplies one. The helper only dials and no
byte is copied inside the namespace; the egress helper is also one-shot. A long-lived helper per run
would avoid a fork per connection and is not measured either way (§ 16, I11).

**Lifetime.** An exec's endpoint is closed when the exec is terminal (`endpoint.closed`); a later
connection is refused by the kernel. A workload's is bound while desired is `running`; during `starting`
and `backoff` a connection that cannot reach the child is closed after accept; on `stopped`, `exited` and
`failed` the listener is closed; on `start` it binds again, to the same port if free, else another, which
the workload's record shows. A connection ends with its run's namespace.

### 5.3 Exposure

`loopback` is `127.0.0.1` and a kernel-chosen port. `node` is refused unless the daemon is given
`--endpoint-node-address <ip>`, a non-loopback, non-wildcard address of the host; without it the refusal
is `endpoint.exposure-unserved` (501) naming `node`. With it, the endpoint binds that address and a
kernel-chosen port. Anyone who can reach it can connect: the control API's subject check does not extend
to that TCP port. The relay forwards no peer address; the service sees loopback.

### 5.4 Bounds

`MAX_ENDPOINTS_PER_RUN` (8, the aperture bound), `MAX_ENDPOINTS_PER_DEPLOYMENT` (64),
`MAX_ENDPOINT_CONNECTIONS` (64) per endpoint and `MAX_DEPLOYMENT_ENDPOINT_CONNECTIONS` (512). A
connection over a bound is closed at accept and counted as `refused_connections` on the endpoint. There
is no byte bound and no idle timeout; the run's end and the endpoint's close end a connection.

### 5.5 The wire of slice 3 (`0.32.0`)

- Facts: `endpoint.loopback`, published only after the mechanism ran once on a throwaway sandbox (a
  connection delivered, the namespace left with no interface); `endpoint.node`, only where
  `--endpoint-node-address` is set and `endpoint.loopback` was proved.
- Fields: the endpoint resource; `endpoints` on the workload spec. Applied: the observed address;
  `exec.no-egress` unchanged.
- Refusals: `endpoint.unserved` (501), `endpoint.exposure-unserved` (501), `endpoint.target-not-found`
  (404: absent, another subject's or terminal), `endpoint.port-invalid` (422), `endpoint.limit` (429),
  `endpoint.bind-failed` (500, nothing left).
- Acceptance, from `story:inbound-endpoint-for-a-confined-exec`:
  - the ADR is accepted and indexed in `adr/README.md`, and `cargo xtask check-adrs` passes;
  - a successor bundle publishes the fact, field, applied record and refusals, and `cargo xtask
    check-bundles` and `cargo xtask check-json` pass with it in `scripts/gate.sh`;
  - portable lane: the fact is absent and a start naming an endpoint is refused by name before dispatch;
  - delegated lane: a confined exec serves one declared port; a client on the host connects, sends a
    request and reads the child's answer; the same child still cannot open an outbound connection to an
    undeclared destination; the endpoint is gone after the exec ends;
  - `exec.no-egress` is unchanged and still true for that exec;
  - from the consumer (weft test `ports-node-endpoints-local`): expose port 8080 of a confined exec (and,
    once slice 2 lands, of a workload) with `exposure: loopback`: the answer carries an observed
    `127.0.0.1:<port>` and an HTTP GET to it returns the child's response; `exposure: node` on a daemon
    whose configuration does not allow it is refused by name; after the exec ends (or the workload
    stops) a connection to the observed address is refused.

## 6. Local endpoints (gap 3)

The consumer's node agent must tell which run a connection came from. A **local endpoint** is an
operator-declared aperture whose destination is a directory of Unix sockets served by that agent. It is
decided, not asked, because it is an aperture (ADR 0013): declared in configuration, selected by name,
never a destination in a request, observed in the record.

### 6.1 The declaration and the fit to the existing shapes

`--local-endpoint <name>=<port>:<directory>` declares one. It shares the namespace of the TCP apertures,
the `aperture` and `apertures` start members, the run bound (8), the deployment bound (12, counting
both) and the addresses `127.0.0.1` to `127.0.0.8` by position. `exec.aperture-undeclared` answers an
undeclared name; a run that names none has no listener.

The existing shapes (§ 1) take it with these changes in the successor bundle, each additive:

| shape | local endpoint |
|---|---|
| fact `exec.egress-apertures` element `{name, destination, max_bytes?}` | listed with `destination: "local:<port>"`; the client reads the port there as it reads a TCP aperture's. Listed only after the local mechanism was verified |
| applied aperture `{mode, name, destination, mechanism, bytes, listen?}` | `mode: "aperture"`; `destination: "local:<port>"` (3 to 64 characters: it fits); `mechanism`: the schema's one-value `const` becomes an enum of `loopback-forwarder` and `local-endpoint-forwarder`; `bytes` counted in the forwarder as today; `listen` always present, also on the single (non-array) form, where the schema gains it as an optional member that only this mechanism fills; `key`, the socket's name in the directory (§ 6.2), a string the caller chose or the run id. **No host path appears**: not the directory and not the socket's path |
| `max_bytes` | not declarable in the first slice |
| start sandbox | the optional `local_endpoint_key` beside `aperture` and `apertures` (§ 6.2) |

A client pinned to an earlier bundle cannot select a local endpoint and so never reads the new values.
The directory is opened at startup and held by descriptor; it must be owned by the daemon's uid and
carry no group or other write permission, or the declaration is refused at startup. ADR 0054 decision 2
accepts no other uid, and makes the directory one of the daemon's private host paths.

### 6.2 The relay

The listener is installed as an aperture's: the helper joins the namespace, binds `127.0.0.n:<port>`,
hands the listener out. The forwarder connects to **`<directory>/<key>`** when the start names
`sandbox.local_endpoint_key`, and to **`<directory>/<run id>`** when it does not, the run id being the id
of the exec (for a session, of the exec it runs) or workload that owns the namespace (at most 32 characters); a workload's is stable
across restarts. The key is 1 to 64 bytes matching `^[a-z0-9][a-z0-9-]*$`; every run id holds `_`, so a key
never names another run's id. The stakeholder decided the key on 2026-11-28: an instance's first call
often comes at startup, the socket named by a minted id may not exist yet, and the caller knows its
instance's id before the start, so its agent creates the socket first. A key held by a live run on the
same local endpoint is refused `exec.local-endpoint-key-in-use` (409); it is free again when that run
ends. The connect is made from the daemon's side through the held directory descriptor, as
`/proc/self/fd/<n>/<name>` in a relay forked from the forwarder, which never leaves the host namespaces
and which no child can reach (the warning of `bind_args` in `process.rs` is about a path given to
bubblewrap). The entry is created and replaced by the agent, which the operator declared and which owns
the directory; the daemon creates and removes nothing there. **If the socket is absent the connection is
closed**; no other socket is tried. § 16, I10 is answered by ADR 0054 decision 4: the startup probe
connects that way before the fact is published.

### 6.3 The wire of slice 4 (`0.30.0`)

Slice 4 landed first, so it publishes `0.30.0` (ADR 0054 decision 10), not the `0.33.0` § 2 proposed.

- Facts: none new; § 6.1. Fields: the optional `sandbox.local_endpoint_key` on both starts; the applied
  `key`. Refusals: `exec.local-endpoints-unserved` (501, a name of a local endpoint selected while the
  mechanism is not verified), `exec.local-endpoint-key-without-endpoint` (422, a key while no local
  endpoint is selected), `exec.local-endpoint-key-in-use` (409, a key a live run holds on the same local
  endpoint); the existing aperture refusals.
- Acceptance, from `story:local-endpoint-attributed-to-its-run` (weft test `ports-node-endpoints-local`):
  - two runs naming the same local endpoint each connect to their listener; the agent's socket for run A
    receives only A's connection and the socket for run B only B's;
  - a run that names no local endpoint has no listener for it;
  - with the agent's socket for a run absent, that run's connection is closed and no other run's socket
    receives it;
  - the applied record names the endpoint and listen address and carries no host path;
  - portable lane: without the mechanism the fact is absent and a start naming a local endpoint is
    refused by name before dispatch.

## 7. Volumes (gap 6)

A volume is a named host directory, owned by the daemon, that outlives a run (design 01 § 3.5, ADR 0038's
mount rules): a design change, not an architecture one.

### 7.1 The resource

`POST /v1/volumes` (`volumes.create`) `{name, labels?, quota_bytes?}`; `GET /v1/volumes[/{id}]`;
`DELETE /v1/volumes/{id}` (`volumes.delete`). Id `vol_...`. A record states `state` (`available`,
`deleting`), `quota_bytes` when set, `used_bytes` when the file system reports it.

- `--volume-root <directory>`, owner-private like the image store. Without it `volume.directory` is
  absent and a create is `volume.unserved` (501). Data is `<root>/<vol id>/data`, mode 0700, the daemon's
  uid; in a run the files are the mapped uid, which is the daemon's (§ 16, I9).
- `quota_bytes` uses a delegated project quota (`quota.rs`, ADR 0020). The fact `volume.quota` is
  published only where the volume root proves one; otherwise a create naming a quota is
  `volume.quota-unserved` (501), as design 01 § 10 decision 6 draws the line.
- At most `MAX_VOLUMES_PER_SUBJECT` (32) and `MAX_VOLUMES_PER_DEPLOYMENT` (128), `volume.limit` (429).

### 7.2 Mounting

Optional `mounts: [{volume, target, read_only}]` on the exec start, session start and workload spec, at
most `MAX_VOLUME_MOUNTS` (8, the writable roots' figure). The target is an absolute view path under the
lexical rules of ADR 0038 and ADR 0010, and not at or beneath `/proc`, `/dev`, `/tmp`, `/workspace`,
`/runtime` or `/.substrate-exec-gate`; else `volume.mount-invalid` (422). The driver pins the data
directory by descriptor at admission and binds it with `--bind-fd` (`--ro-bind-fd` when `read_only`), so a
swapped path redirects nothing. The same volume may be mounted by several runs; the daemon takes no
cross-run lock. A target beneath an image entry follows § 3.5.

### 7.3 Delete

Refused `volume.in-use` (409) while a non-terminal exec or session, or any workload record naming the
volume, mounts it; a workload's record holds it until the workload is deleted, because its next start
mounts it. Otherwise `deleting` is durable, the directory is renamed to a trash sibling and removed by a
bounded walk that follows no link, and the record goes; a daemon restart resumes a `deleting` volume.

### 7.4 The wire of slice 5 (`0.34.0`)

- Routes: § 7.1. Facts: `volume.directory`, `volume.quota`. Fields: `mounts`. Applied: `mounts`, each
  `{volume, target, read_only}`, no host path.
- Refusals: `volume.unserved`, `volume.quota-unserved` (501); `volume.mount-invalid` (422);
  `volume.not-found` (404); `volume.in-use` (409); `volume.limit` (429).
- Acceptance, from `story:read-write-volume-kept-across-runs` (weft test `ports-node-volumes-local`):
  - create a volume: it answers `available`; run A mounts it read-write at `/data` and writes a file;
    after A ends, run B mounts it and reads the file;
  - delete while B runs is refused `conflict`; after B ends, delete succeeds and the directory is gone
    from the host;
  - a run mounting it read-only cannot write to it;
  - portable lane: without the mechanism the fact is absent and a create is refused by name.

## 8. Per-run secret values (gap 4, D4)

Secret slots are names that resolve from daemon configuration (§ 1). This adds a value in the request. It
is last because **it contradicts design 01 § 7.7** ("No secret values in ordinary JSON, ever"): option A
amends that rule to one member, `secret_values`, on the Unix-socket posture only, and § 7.7's catalog
reading is the cost. Option B also puts a value in JSON; only C keeps § 7.7 and it does not serve the
consumer.

### 8.1 The member

`secret_values: [{slot, fd, value}]` on the exec start, session start and workload spec, in the shape of
`secret_slots` plus a `value` string of 1 to `MAX_SECRET_SLOT_BYTES` bytes. Both lists share the run's
bound of 8 and one namespace of slot names; a name in both is `exec.secret-value-name-taken` (422).

### 8.1.1 Where a value lands and where it does not

Read from the tree (§ 1); each row is a requirement of the slice and a case in § 12.

| place | outcome |
|---|---|
| `operations.request_hash` | **excluded.** The hash is an unkeyed SHA-256 over the raw input, stored; a value in it is an offline oracle for any low-entropy secret. Every `secret_values[*].value` is removed from the JSON before `canonical_request_hash_v2` is called; the slot and fd stay. A start without the member hashes as on `0.29.0` |
| `operations.outcome_json`, `execs.resource_json` | none: the applied record echoes `{slot, fd}` as `secret_slots` does (a name and a number, never a value or a length) |
| the ledger's input | the `operations` table has no input column; nothing to redact there |
| events, the log ring, errors | none: an error may name a slot (design 04 § 6) and never a value; `workload-transition` has no `env` |
| daemon memory | the value is in the request body buffer, the parsed JSON value and a dedicated buffer. Only the last is zeroed on drop (`zeroize` is in the lock); the first two are ordinary heap. **A core dump or swap of the daemon holds the value**; this is a stated residual (§ 17) |
| the child | a sealed memfd at its descriptor, as a slot's (`memfd.rs`), named by `SECRET_SLOTS_ENV` (names and numbers only) |
| `workloads` table | none; the value is not stored (§ 8.2) |

### 8.2 Behaviour

- **A changed value on a retry is detected without storing it.** At admission the daemon computes for each
  slot `HMAC-SHA256(k, op ‖ slot ‖ value)`, where `k` is 32 bytes from the OS random source taken at daemon
  start and held in memory only, and keeps the result in memory beside the run's `DriverRun` accounting,
  by scope and operation, until the run is terminal or the workload deleted. A retry whose operation
  already has a ledger row compares its commitments in constant time before the replay is answered
  (the branch of `decode_mutation` that reads the reservation); a difference is
  `operation.request-conflict` (409), the code the ledger already gives for different input. A
  per-daemon key means a leaked commitment is no oracle after the process, and the commitment is never
  persisted, logged or answered. `hmac` is not in the lock; HMAC over `sha2` is a short construction
  or one new crate (§ 16, I2). **After a daemon restart the map is empty**: a retry then replays the
  recorded answer without comparison. That is accepted: the run it describes is gone (an exec died, a
  workload is `failed`, below), and nothing is delivered by a replay.
- **A retry resends the value.** Nothing at rest holds it; a retry that must dispatch and has none is
  `exec.secret-values-required` (422). A retry that finds the operation recorded replays and does not
  read the value.
- **A workload's value lives in memory for its life**, in sealed memfds, so a policy restart gets the
  same value. After a **daemon** restart it is gone: the relaunch of § 4.6 cannot happen, the workload is
  `failed` with `cause: start-failed` and code `workload.secret-values-lost`, desired stays `running`, and
  the caller deploys again with the values. A lease expiry closes the memfds.
- **Only over a Unix socket.** A request carrying `secret_values` over the static-bearer TCP development
  transport is `exec.secret-values-transport` (422) before dispatch. The fact is published only on a
  daemon serving a Unix socket that proved sealed delivery on a throwaway run; absent otherwise, and a
  start naming one is `exec.secret-values-unserved` (501).

### 8.3 The wire of slice 6 (`0.35.0`)

- Fact: `exec.secret-values`, `const true`. Fields: `secret_values`. Applied: `secret_values: [{slot,
  fd}]`.
- Refusals: `exec.secret-values-unserved` (501), `exec.secret-values-transport` (422),
  `exec.secret-value-name-taken` (422), `exec.secret-values-required` (422), `operation.request-conflict`
  (409, existing), `workload.secret-values-lost` (409 on `start`, and the event's code).
- Acceptance, from `story:a-start-delivers-a-per-run-secret-value` (weft test
  `ports-cloud-secret-store-local`):
  - a start carries one value; the child reads exactly that value once from its descriptor;
  - two runs started with different values each read their own;
  - the value's bytes appear in no response, record, event, log stream, ledger file or daemon log,
    checked by searching every byte the daemon wrote and answered during the run;
  - a start naming a per-run secret over TCP development transport, or with the mechanism absent, is
    refused by name before dispatch.

## 9. Gap 7 is already served

Bundle `0.21.0` has it: the optional `apertures` list beside `aperture`, the array form of the applied
network and `exec.aperture-undeclared` ([design 26](26-working-directory-and-several-apertures.md)); the
bounds are eight per run and twelve per deployment (ADR 0013, amendment of 2026-11-20). A workload names
apertures by the same list (§ 4.8, step 7). Nothing is added here.

## 10. What stays true

The enforced isolation set of `AGENTS.md` § Safety envelope is a floor, not a menu. For an image run and
a workload:

| floor item | image run and workload |
|---|---|
| `openat2` beneath, no-link I/O; atomic replacement | holds; the unpack, the volume layer and the store publish use it (§§ 3, 7) |
| cleared and shaped environment | holds (`--clearenv`); `PWD` is the one variable bubblewrap sets (§ 3.5) |
| namespace no-egress | holds; `exec.no-egress` stays `true` with an endpoint (§ 5.2) |
| no nested user namespace; the seccomp program with the ioctl refusals of ADR 0050 | holds; the same `USER_NAMESPACE_ARGV` and `--seccomp` |
| pids and memory+swap bounds | holds; the same cgroup writes |
| cumulative CPU and timeout | **not applied to a workload**, which runs until stopped; its CPU is the `cpu.max` rate, its bound the lease and the stop (W9). Applied to an exec with an image as to any exec |
| backend-identity-bound snapshots | holds; a relaunch re-checks (§ 4.6) |
| output draining; whole-tree kill | holds; the ring and `cgroup.kill` with the emptiness proof |
| read-only `/runtime` beside writable `/workspace` | holds for an exec; a workload has no workspace in the first slice |
| owner-private durable state; bounded cleanup | holds; the store and the volume root are mode 0700 and opened by descriptor; staging, trash and `deleting` are bounded and fail closed |
| host roots read-only | an image view **has no host roots**: the host's `/usr` and `/lib` are not in it; a declared root of ADR 0010 still binds under § 3.5's mount-target rule |

Weakening any is invariant 3's named refusal, never a quiet downgrade. The new trust: the image tree is
untrusted content the run executes, read-only, owned by the daemon's uid, with no setuid. The new
surfaces are the unpacker (untrusted bytes in the daemon), the relay's helper (a process in a run's
namespace with no destination of its own), the secret value (§ 8.1.1) and the TCP port of a `node`
endpoint (unauthenticated, § 5.3).

## 11. Consumer compatibility

A client pinned to `0.29.0` or earlier reads nothing new: every member is optional, every route new,
every fact optional, and no bundle holds a route or word that moves (§ 1). A start without `image`,
`mounts` or `secret_values` serializes and hashes as on `0.29.0`, held by a vector on a stored hash as
design 37 holds its own. For `weft` each slice is a fact to read and then a route to
use; a missing fact is a named refusal, never a degraded run.

## 12. How it is proved

Bounds: the test step ends within 120 s, no test runs over 60 s, no job over five minutes, a test over
5 s is named with its time (`AGENTS.md` § The gate). A soak or load run is local evidence, not a test.

| lane | cases |
|---|---|
| portable | every new fact absent; each start or create of a slice refused by its code before dispatch and recorded as refused; the lexical rules (`image.digest-invalid`, `endpoint.port-invalid`, `volume.mount-invalid`) before the fact, as design 37 orders its own |
| delegated | slice 1: an archive created in chunks and an exec reading `/etc/os-release` from it, a digest mismatch leaving nothing, a setuid entry stored without the bit, delete while held and after; slice 2: every line of § 4.9, the restart policies on a program the test supplies, a lease expiry, the daemon-restart case with a second daemon on the same state, the lane's isolation cases against a workload; slice 3: the HTTP round trip, the no-egress case, the endpoint's closing; slice 4: two runs and two sockets; slice 5: two runs and a volume; slice 6: the byte search of § 8.3 over the daemon's state, answers and log, and the changed-value retry |
| namespace | every new fact absent in the probed snapshot |
| unit | the unpacker on archives the test builds (`..`, absolute path, device, FIFO, socket, duplicate, hard link to a later entry, symbolic-link parent, long path, an inflate bomb past its bound); the state machine on a test clock; the request hash with and without a value; the commitment compare |

- No test makes a model call, a network request or a registry request. A fixture image is built once per
  run by a setup step, never per test: the lane builds one tiny static program for the host's musl target
  as it builds the exec-gate helper and packs it with the test's own tar writer.
- The totals `AGENTS.md` states move in the change that adds a case; this document states no new total.
- Daemon-source and wire changes run `driver_port` and the three crates' library tests (`AGENTS.md` § The
  gate); each slice adds its bundle check to `scripts/gate.sh`; slice 1's dependencies pass
  `check-licenses`, `check-advisories` and `check-packages`.

## 13. Order of work

1. The operator's session accepts or revises this document and answers D1 to D4. An ADR per question
   records the answer before code; gaps 3 and 6 amend ADR 0013 and design 01 § 3.5, D4 also § 7.7.
2. Slices in the order of § 2; slices 3 and 4 may begin beside slice 2 for execs. Each slice is a story
   that starts when its critics approve it, and its wave report names the consumer gap it moved.

## 14. Decisions

| id | decision |
|---|---|
| W1 | `workloads` is a family of the host driver; one fact `workload.supervised`, published after a throwaway image-view run. Not Docker-backed. |
| W2 | The first slice serves deploy, get, list, stop, start, restart, delete, a bounded log read and lease renewal. Replace is not served; `workload.replace` is not published. |
| W3 | Seven observed states; desired is `running` or `stopped` and durable before dispatch. |
| W4 | Back-off 1,000 ms base, 60,000 ms maximum, stable run 30,000 ms, limit 100; a start resets. Proposals, held by cases on a test clock. |
| W5 | A lease-expiry stop is a daemon-originated ledger operation, as for a host process. A restart by policy and a relaunch are events and `restarts`, not operations; their event cause is the operation that last put the workload at desired `running`. |
| W6 | A daemon restart relaunches a workload whose desired state is `running`, whatever its policy, after the old cgroup is proved removed. A workload does not survive its daemon. |
| W8 | Lease expiry stops a workload and the record stays; delete is refused `workload.not-stopped` while a process may exist. |
| W9 | A workload has no `timeout_ms` and no cumulative `cpu_millis`; its limits are `processes`, `memory_bytes` and a rate `cpu_millicores`; its bound is the lease and the stop. |
| W10 | A workload's log is one 8 MiB ring across restarts, read by a bounded `GET`. |
| W11 | An archive is received in chunks, hashed and compared before it is parsed. |
| W12 | The unpack runs in the daemon process with a tar reader written here and `flate2` over `miniz_oxide`. |
| W13 | Entry rules of § 3.3: four kinds, no duplicates, links stored and never followed, hard links only to earlier regular files, setuid and setgid dropped, owners and times not kept, xattrs ignored. |
| W14 | A content-addressed store under `--image-store`, per-digest tree, per-subject record, publish by `rename_noreplace`, holds in a table. |
| W15 | The image view binds each top-level tree entry read-only onto bubblewrap's tmpfs root, with `/proc`, `/dev`, `/tmp` and the fixed binds after; no `env` stage; `PWD` in `env` refused; a mount target beneath an image entry must exist; executable lists with an image are refused. |
| W16 | Inbound: a host listener, a helper per connection that joins the namespace, connects to `127.0.0.1:<port>` and hands the socket back, and a host relay. `exec.no-egress` stays true. |
| W17 | The exposure word is `node`, renaming design 01's `lan`; no released bundle holds either. |
| W18 | A local endpoint is an aperture of a second mechanism, selected through `aperture`/`apertures`, listed in `exec.egress-apertures` with `destination: "local:<port>"`, applied with `mechanism: "local-endpoint-forwarder"` and a `listen`; the socket is `<directory>/<run id>`; an absent socket closes the connection. |
| W19 | A volume is `<volume-root>/<id>/data`, bound by descriptor, optionally quota-bounded; delete is refused while mounted or named by a workload. |
| W20 | A secret value is in the request, memory only, in a sealed memfd, over a Unix socket only. It is removed from the JSON before the request hash. A changed value on a retry is detected by an in-memory HMAC commitment under a per-daemon key, and not after a daemon restart. |
| W21 | The archive travels as keyed chunk operations of at most 1 MiB inside the existing envelope; a raw upload is not in this design. |
| W22 | A workload's `env` is stored at rest and declared non-secret. |
| W24 | Six bundles, one per slice, in the order of § 2; the numbers are proposals. |

## 15. Questions with more than one answer

Each is for the operator, who puts it to the stakeholder, the session that maintains `weft`. The first
option is the one this document is written with. None of the four has an answer yet.

**D1. Which family serves the workload?**

- **A. A `workload` family on the host driver, confined, daemon-supervised.** § 4.
- B. Docker-backed workload only: a container daemon per node.
- C. No family: the consumer re-starts long execs.

B is the planned `story:docker-image-backed-workload-slice` and is root-equivalent to its host (design 04
§ 1). C makes the consumer the supervisor, and a daemon restart then kills every service with nobody to
relaunch it.

**D2. How is a root filesystem unpacked?**

- **A. The daemon unpacks a gzip tar in Rust.** § 3.
- B. An external unpacker tool: a new host dependency.
- C. Images only through the `oci` adapter: a container daemon.

A puts untrusted bytes in the daemon and adds two dependencies. B moves the parser out and the
descriptor-bound creation of § 3.3 no longer holds. C depends on the operator's container runtime (design
04 § 1: root-equivalent).

**D3. How does a connection reach a confined run?**

- **A. A host-side relay like the egress aperture.** § 5.
- B. A virtual ethernet pair with nftables, inbound only: needs `CAP_NET_ADMIN`, and `exec.no-egress`
  becomes false.
- C. `pasta` or `slirp4netns` port forwarding: a host dependency and a tap interface.

B and C give the child an interface, so `exec.no-egress` cannot stay `true`; invariant 3 refuses a silent
downgrade and each needs a new fact stating the narrower guarantee.

**D4. How does a per-run secret reach the run?**

- **A. A value in the start request, memory only.** § 8.
- B. A write-only route that stores it on the node.
- C. Daemon-configuration secrets only.

A and B both put a value in JSON against design 01 § 7.7; A amends it for one member. B also puts a secret
at rest on the node. C keeps § 7.7 and is today's state; it does not serve the consumer, whose store gives
a value per run.

This document is written with A for each of D1 to D4.

## 16. Inferred, not observed

**Closed in the second revision**, each now a cited claim above: I1 (no bulk route exists: § 1, § 3.2);
I4 (the refusal stays, with its reason read: § 3.5); I5 (`node`: § 5.1); I6 (the applied element read in
the wire crate and the `0.29.0` schema: § 6.1); I7 (the ledger stores a hash, a state and an outcome, no
input: § 8.1.1); I8 in part (`rename_noreplace` and `openat2` are used in the tree; `linkat` is not, below);
I12 (the event union read: § 4.7); I13 (409 and 404 read: § 1); I14 (`ExecLimits` read; the first revision
was wrong that it holds a rate: § 4.1).

Still inferred:

- **I2.** Licences of `flate2`, `miniz_oxide` and `tar` were read from the registry copies on this host
  and match `about.toml`. Advisories were not checked (`check-advisories` clones the RustSec database
  over the network). That `check-packages` and the reproducible-build checks accept a new dependency was
  not run. Whether to add the `hmac` crate or write HMAC over `sha2` is open.
- **I3.** That bubblewrap accepts a root made only of `--ro-bind-fd` binds on its tmpfs, creates the
  mount points, and allows a bind over a file inside a read-only bind is inferred from the argv `command`
  builds. It was not run and the bubblewrap source was not read. The probe of § 3.6 proves it; the fact
  is absent where it fails.
- **I8.** `linkat` for hard links is used nowhere in the tree; that it works on descriptors on the target
  kernels is inferred.
- **I9.** That the run's mapped uid is the daemon's, so a service that insists on root ownership fails
  and the daemon can remove a volume's files, is inferred. The tree passes no `--uid` to bubblewrap
  (§ 1); bubblewrap's default mapping was not read or run.
- **I10.** That a `connect` through `/proc/self/fd/<n>/<run id>` from the daemon reaches the agent's
  socket as intended, and that a `sun_path` of that length is accepted, is inferred. Not run.
- **I11.** That one forked helper per inbound connection is fast enough for a service is inferred and not
  measured; the egress helper is one per aperture, not per connection.
- **I15.** That root file systems exported by common tools stay inside the pax and GNU extensions of § 3.3,
  and that `cpu.max` from `cpu_millicores` behaves as `exec_cpu_quota` does with no wall limit, are
  inferred; no such archive was read and no such run made.
- **I16.** That the exec-gate helper starts with no `env` stage in the view and so image runs with a list
  would not need one is read in design 35 D5 and D29 text, not run.

## 17. Not decided here

- Whether a workload survives a daemon restart (W6 relaunches). Surviving needs the host-process
  supervisor's pidfd re-attachment under an exec's confinement and no `--die-with-parent`.
- Whether the unpack runs in a child under seccomp and an empty view, so a parser bug does not reach the
  daemon.
- Executable lists with an image: amend design 35 D3, D24, D30 for image binds, and the driver's
  judgement away from `BASE_MOUNTS`.
- A raw-body archive upload, which needs an exception to design 01 § 7.1.
- Zeroing or locking the request buffers that hold a secret value, and whether the daemon should
  `mlock` or disable core dumps.
- OCI images as a second input, registries, image build and push, `workload.replace`, a WebSocket log
  stream, a tunnel, a workspace mounted into a workload, `max_bytes` on a local endpoint.
- Metrics for workloads and endpoints (ADR 0045), the Kubernetes namespace driver, the Docker driver,
  and placement of a workload on more than one host.

## 18. Not read

- Design 35 beyond D1 to D5, D20, D22, § 5 and the part of § 6 cited; design 09 and the hosted Identity
  profile beyond what `AGENTS.md` says; design 27; the planning-store stories beyond their acceptance and
  consumer-requirement sections.
- The bubblewrap manual and source; every claim about its mount-point creation and uid mapping is § 16.
- The consumer's own document, `cloudspec` `docs/platform-ports.md` § 10 at `2ed72c2`: the gaps are taken
  from the epic and the stories that quote it.
- The host-process tests and `ports.rs` (the supervisor shape is taken from ADR 0036 and the module
  headers); the macOS and Kubernetes adapters, where none of this design is served.
