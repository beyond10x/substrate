---
status: accepted
date: 2026-11-28
---

# ADR 0055: the daemon unpacks a root filesystem from a sha256-named archive

Decider: the operator, 2026-11-28 11:08 CET, relayed by the operator's session (D2, option A);
`approval-record:operator-answers-design-38-questions-2026-11-28`. Carries
[design 38](../docs/design/38-long-lived-service-on-the-host-driver.md) § 3 and decisions W11 to
W15 and W21, **with one amendment of design 38 § 3.4**: the store layout of decision 6 and two bounds
of decision 4 are what the first implementation found (commit `5e70016d46` on branch
`wave/2026-11-28-rootfs-unpacker`), and they replace the design's layout. The design comes before code (invariant 8): this record adds a resource family, a
capability fact, optional start members, events, refusal codes and a successor bundle, and no code
yet. Bounds are proposals the first slice holds with cases; a figure here moves only through an
amendment.

## Context

The `weft` repository runs a long-lived service from a root filesystem on a workstation with no
container daemon. The host driver cannot give a run a root of its own today:

- The base view binds the host's `/usr` read-only, `/bin`, `/lib`, `/lib64` and `/etc/alternatives`
  when present, then `/proc`, `/dev` and a `/tmp` tmpfs; the rest of the root is bubblewrap's tmpfs,
  and without the exec-gate helper the program runs as `/usr/bin/env -u PWD -- <argv>` (`command` in
  `crates/substrate-host/src/process.rs`; [ADR 0049](0049-the-base-view-carries-etc-alternatives-and-tmp-takes-an-operator-size.md)).
- No released bundle holds a route for images (`contracts/substrate-wire/0.29.0/operations.json`),
  and design 01 § 8 marks Image unserved on the host driver.
- The daemon has no bulk route. Every mutation reads a body of at most `BODY_LIMIT` (2,097,152
  bytes) within `REQUEST_BODY_READ_TIMEOUT` (10 s) as an `{op, input}` JSON envelope (`app.rs`,
  `read_bounded_body` and `decode_mutation` in `app/operations.rs`). Bytes ride JSON as base64: a
  file is at most `MAX_FILE_BYTES` (1,048,576, `crates/substrate-wire/src/lib.rs`). Design 01 § 7.1
  makes one operation one method, path and JSON body.
- The lock file has `sha2`, `zeroize` and `getrandom` and no tar or inflate crate (`Cargo.lock`).
- The guarded file layer exists: `openat2` with `RESOLVE_BENEATH`, `RESOLVE_NO_SYMLINKS`,
  `RESOLVE_NO_MAGICLINKS` and `RESOLVE_NO_XDEV` (`crates/substrate-guarded/src/fs.rs`), and
  `rename_noreplace` (`crates/substrate-host/src/rename.rs`). `linkat` is used nowhere in the tree.
- Roots are bound into a run by descriptor (`--ro-bind-fd`), never by `/proc/self/fd/<n>` as a path
  ([ADR 0010](0010-declared-host-roots-are-mounted-read-only.md),
  [ADR 0038](0038-writable-roots-are-named-host-paths-or-git-checkouts.md)).

The consumer ships a root filesystem as a gzip tar. A run that mounts its own tree read-only as `/`
needs the tree on the host, made by something that does not trust the archive.

## Decision

1. **An image is identified by the sha256 of its archive bytes**, `sha256:` and 64 lower-case hex
   digits (design 01 § 1: images go by digest). A record states `digest` (the observed digest),
   `size_bytes`, `entries`, `unpacked_bytes`, `state` (`receiving`, `available`) and `created_at`.
   The record is per subject; the unpacked tree is per digest and shared. A record of another subject
   is the same `image.not-found` as an absent one (design 04 § 9).

2. **The archive is received in chunks inside the existing envelope** (W21). A raw-body upload would
   be a new route class that breaks design 01 § 7.1, and is not decided here.

   | route | operation | notes |
   |---|---|---|
   | `POST /v1/images` | `images.create` | `{digest, size_bytes}`; answers `receiving`; durable before any byte is received ([ADR 0005](0005-operations-are-durable-before-driver-dispatch.md)) |
   | `POST /v1/images/{digest}/chunks` | `images.chunk` | `{offset, content}`, `content` base64 of at most `MAX_IMAGE_CHUNK_BYTES` (1,048,576, the figure of `MAX_FILE_BYTES`); `offset` must equal the bytes received, else `image.chunk-out-of-order` (409); answers `{received_bytes}` |
   | `POST /v1/images/{digest}/complete` | `images.complete` | verifies size and digest, unpacks, answers the record `available` |
   | `GET /v1/images[/{digest}]` | read | |
   | `DELETE /v1/images/{digest}` | `images.delete` | `image.in-use` (409) while held |

   Every mutation is keyed; a retried chunk replays by operation id.

3. **The digest is compared before anything is parsed** (W11). Chunks append to a file in an
   owner-private staging directory while the daemon computes the sha256. `complete` with a received
   size other than `size_bytes` is `image.size-mismatch` (422). A digest other than the named one is
   `image.digest-mismatch` (422), which states the observed digest; the staging file and the record
   are removed and nothing else exists.

4. **The unpack rules** (W13). The daemon inflates and reads the tar from the staging file and
   creates each entry in an unpack directory of the store (`trees/.unpack-<ulid>`, decision 6). Each
   rule is `image.entry-refused` (422) unless another code is stated; a refusal removes the unpack
   directory.
   - Kinds: regular file, directory, symbolic link, hard link. Device, FIFO, socket, sparse and any
     other kind are refused.
   - Paths are relative and a leading `./` is dropped. Absolute, empty, NUL, a `..` component, a
     component over 255 bytes, a path over `MAX_IMAGE_PATH_BYTES` (4,095) and a path named twice are
     refused.
   - Every entry is created with `openat2` beneath the unpack root, no symlink and no magic link
     followed, through `substrate-guarded`'s layer. An entry beneath a symbolic link already created
     is refused.
   - **A symbolic link is stored and never followed.** Its target text, at most
     `MAX_IMAGE_LINK_TARGET_BYTES` (4,095), is not interpreted; an absolute or escaping target is an
     ordinary stored link that resolves in the run's own root.
   - A hard link is made only to a regular file the same archive already unpacked, with `linkat` on
     descriptors; any other target is refused.
   - Modes keep the permission bits with `setuid` and `setgid` dropped and the sticky bit kept; write
     bits are cleared on the finished tree. Owners and times are not kept: every entry is the daemon's
     uid. Extended attributes, file capabilities and ACLs in pax records are ignored.
   - Bounds, counted while inflating so a small archive that inflates past one stops at it, each
     `image.archive-limit` (429): archive bytes `MAX_IMAGE_ARCHIVE_BYTES` (104,857,600, design 01
     § 10's ceiling), unpacked bytes `MAX_IMAGE_UNPACKED_BYTES` (1,073,741,824), entries
     `MAX_IMAGE_ENTRIES` (65,536). The directory paths an unpack keeps are
     bounded too, by `MAX_IMAGE_DIRECTORY_PATH_BYTES` (16 MiB, summed). An entry whose stated size
     exceeds the unpacked bound is refused at its header, before its bytes are read. The deployment
     may lower the first three; the fact states the effective value.
   - Pax `path`, `linkpath` and `size` and the GNU long-name and long-link entries are honoured; other
     extensions are ignored. A malformed header, a short read, a bad checksum, or a gzip stream that
     does not end with the tar is `image.archive-invalid` (422).

5. **The unpack runs in the daemon process** (W12), in the host crate, which the daemon reaches only
   through the `Driver` port (invariant 4). The tar reader is written in this repository, so every
   file is created through the descriptor layer and not through a crate's own path handling. The
   inflate is `flate2` with its pure-Rust `miniz_oxide` backend. Both are new dependencies. Their
   licences, read from the registry copies on the design's host, are inside `about.toml`'s accepted
   set; the slice passes `cargo xtask check-licenses`, `check-advisories` and `check-packages`
   before merge.

6. **The store** (W14). `--image-store <directory>` names it, with no default and no environment
   variable (design 37's `--exec-gate` precedent). Without it the fact is absent and `images.create`
   is `image.archive-unserved` (501) before any byte. The directory is opened at startup and must be
   owned by the daemon's uid with no group or other permission; otherwise the fact is absent with a
   diagnostic.
   - **Layout** (this amends design 38 § 3.4, which had a tree unpacked under `staging/` and moved
     into `trees/`). A finished, read-only tree root cannot be renamed into another parent, because
     the kernel needs owner write on a directory moved across parents. The store is therefore:
     `staging/<ulid>/` holds only the archive being received; a tree is unpacked as
     `trees/.unpack-<ulid>`; the root gets its final mode before the publishing step; and the tree is
     published by one `rename_noreplace` **inside `trees/`**, to `trees/<sha256>/`. A second create of
     an existing digest drops its own staging and unpack. `trash/<random>` is as before.
   - **The root's marker.** `open` writes a marker file `.substrate-image-store` into an empty
     directory and refuses a directory that holds any entry other than the marker, `staging/`,
     `trees/` and `trash/`, so a mistaken `--image-store` never adopts or deletes a directory of
     other content. The fact is then absent with a diagnostic.
   - Startup removes bounded, well-formed `staging/`, `.unpack-` and `trash/` entries and fails
     closed on anything else (design 04 § 3).
   - Two new tables, `images` and `image_holds` (one row per exec, session or workload that uses a
     tree). No existing table changes.
   - Delete of an image held by a run, or named by a workload record, is `image.in-use` (409).
     Otherwise the record goes, and the tree when the last record of its digest goes: renamed into
     `trash/`, removed by a bounded walk that follows no link.
   - A `receiving` record older than `IMAGE_RECEIVE_TTL_MS` (900,000) is removed with its staging
     file. A subject holds at most `MAX_RECEIVING_IMAGES_PER_SUBJECT` (4), `image.limit` (429) beyond.
     The store holds at most `MAX_IMAGES` (64) trees and `--image-store-bytes` of unpacked data when
     given; beyond either, `image.store-full` (429).

7. **The run's view** (W15). An exec or session start gains an optional `image: {digest}`; a workload
   names one ([ADR 0056](0056-a-workload-is-a-confined-run-the-daemon-supervises-on-the-host-driver.md)).
   - The host's `/usr`, `/bin`, `/lib`, `/lib64` and `/etc/alternatives` are not bound. For each
     top-level entry of the unpacked tree the driver binds it read-only at `/<name>` with
     `--ro-bind-fd` on a descriptor pinned at admission, onto bubblewrap's own tmpfs root.
     `/proc`, `/dev`, `/tmp` and the other fixed binds come after, so an image entry of that name is
     shadowed, not an error.
   - A mount that targets a path beneath an image entry (an aperture's `/etc/hosts` and CA bundle, a
     later volume) needs that path to exist in the image, because the image is read-only; else
     `image.mount-target-absent` (422) names the path. A target at a top-level path the image lacks
     needs nothing.
   - An image run starts the program with no `env` stage; `PWD` is bubblewrap's. A start whose
     `env.set` carries `PWD` is `image.env-pwd-unserved` (422).
   - **Executable lists with an image stay refused**, `image.exec-lists-unserved` (501). Design 35 D3
     makes an absolute entry eligible only beneath `/usr`, `/bin`, `/lib`, `/lib64`, `/workspace` and
     declared mounts, and the driver judges those against `BASE_MOUNTS`, "bound from the host's own"
     (`exec_gate.rs`). With an image the view's `/usr` is the image's, so the driver would judge the
     host's place and answer for the wrong file system.
   - A missing program or working directory ends the run as it does today: an exit, not a refusal.

8. **The wire of the slice**, in a successor bundle that adds no route class and rewrites no released
   byte (invariant 6):
   - Fact `image.archive`, `{max_archive_bytes, max_unpacked_bytes, max_entries}`, published only
     after the open proved the store directory, `openat2` beneath and a throwaway image-view run;
     absent otherwise (invariant 3).
   - Optional `image: {digest}` on the exec and session start; the applied record echoes the digest
     the run mounted. A start without it serializes and hashes as on `0.29.0`.
   - Events `image.created` and `image.deleted`.
   - Refusals, each a wire constant with a vector:

   | code | status | when |
   |---|---|---|
   | `image.digest-invalid` | 422 | not `sha256:` and 64 lower-case hex |
   | `image.archive-unserved` | 501 | the fact is absent |
   | `image.chunk-out-of-order` | 409 | decision 2 |
   | `image.size-mismatch`, `image.digest-mismatch` | 422 | decision 3 |
   | `image.archive-invalid`, `image.entry-refused` | 422 | decision 4 |
   | `image.archive-limit`, `image.limit`, `image.store-full` | 429 | decisions 4 and 6 |
   | `image.not-found` | 404 | not a record of the subject |
   | `image.in-use` | 409 | delete while held |
   | `image.mount-target-absent`, `image.env-pwd-unserved` | 422 | decision 7 |
   | `image.exec-lists-unserved` | 501 | decision 7 |

   The lexical refusal is checked before the fact, as design 37 orders its own.

9. **OCI images, build, pull and push stay unserved** on the host driver (design 01 § 10,
   decision 4).

## Alternatives

- *An external unpacker tool (D2 B).* Moves the parser out of the daemon, and the
  descriptor-bound creation of decision 4 no longer holds. A new host dependency.
- *Images only through the `oci` adapter (D2 C).* Depends on the operator's container runtime,
  which design 04 § 1 calls root-equivalent.

## Consequences

- The store root carries a marker and refuses a directory of other content at open, so a wrong
  `--image-store` fails at startup and nothing in it is adopted or removed.
- **Untrusted bytes are parsed in the daemon process.** A parser bug is a daemon bug. The rules of
  decision 4 and the digest-first order narrow it and do not remove it. Whether the unpack moves
  into a child under seccomp and an empty view is not decided here.
- Two dependencies join the lock file. Their licences are inside `about.toml`'s set; advisories were
  not checked at design time, because `cargo xtask check-advisories` clones the RustSec database
  over the network.
- A 100 MiB archive is 100 chunk operations and about as many ledger rows, against the ledger's
  bound of 100,000 rows per subject (`OPERATION_LEDGER_*_MAX_*` in `crates/substrate-wire/src/lib.rs`).
  A raw upload is faster and needs its own exception to design 01 § 7.1.
- **What stays true of the isolation floor.** An image run keeps the unprivileged non-nestable user
  namespace (`USER_NAMESPACE_ARGV`), the seccomp program with the `ioctl` refusals of
  [ADR 0050](0050-the-floor-refuses-the-ioctl-requests-that-exchange-file-blocks.md), the pids and
  memory+swap bounds, the cleared environment and the no-egress network namespace. The tree is
  untrusted content the run executes, read-only, owned by the daemon's uid, with no setuid bit.
  The image view has no host roots: the host's `/usr` and `/lib` are not in it. A declared root of
  ADR 0010 still binds under the mount-target rule of decision 7. Removing or weakening any floor
  item is invariant 3's named refusal, never a quiet downgrade.
- Executable lists cannot be combined with an image until design 35 D3, D24 and D30 are amended for
  image binds and the driver's judgement stops reading `BASE_MOUNTS`.
- `AGENTS.md` invariant 6, the gate section and the lane counts of the runtime vectors move with the
  successor bundle, as with every successor. Design 01 text this contradicts (§ 3.4, § 8) is edited
  with the slice.
- The unpack is made once per digest and never per test: the fixture image of the lanes is built once
  per run by a setup step.

## Not decided here

- Whether the unpack runs in a child under seccomp and an empty view.
- Executable lists with an image.
- A raw-body archive upload.
- OCI images as a second input, registries, image build and push.
- The number of the successor bundle: slices are ordered by the consumer's gaps, so it is taken when
  the bundle is cut.
- Inferred in design 38 § 16 and proved by the slice, not read here: that bubblewrap accepts a root
  made only of `--ro-bind-fd` binds on its tmpfs, creates their mount points and allows a bind over
  a file inside a read-only bind (I3; the fact `image.archive` is absent where the throwaway run
  fails); that `linkat` works on descriptors on the target kernels (I8); that root file systems
  exported by common tools stay inside the pax and GNU extensions of decision 4 (I15); and whether
  the advisory and package checks accept the two dependencies (I2).
