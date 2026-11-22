---
title: Release notes
description: What changed in the newest Substrate release, generated from CHANGELOG.md.
---

# What changed in the newest release

This page ends with the newest released section of `CHANGELOG.md`, and `cargo xtask check-docs`
fails when the two differ. Earlier releases are in the
[full changelog](https://github.com/beyond10x/substrate/blob/main/CHANGELOG.md). The
[confinement adapters](./concepts/adapters.md) page explains each adapter the notes name.

What 0.16.1 first publishes is listed under 0.16.0 in the full changelog and summarized on the
[status page](./status.md#new-in-0161), including the namespace driver's limits.

<!-- Generated from CHANGELOG.md by `cargo xtask docs-releases`. Edit CHANGELOG.md, not the text below. -->

## 0.24.0 (2026-11-22)

### Added in 0.24.0

- `ConfinementProfile` carries an executable deny-list, `exec_denied_paths`, beside
  `exec_subpaths` (design 35 § 8). `linux`, `systemd` around it and `macos` enforce it and prove
  the new fact `exec_denied_paths`, which stands directly after `exec_subpaths`; `local`, `oci`,
  `pod` and `systemd` around `local` refuse the profile `confinement.fact-unserved`. The wire,
  the bundle and the daemon are unchanged, and the fact has no wire property.
  - An entry has the form of an `exec_subpaths` entry, and a denied path wins: over a listed
    directory that holds it, over the loader and over the program. A start whose program is
    denied is `exec.sandbox-program-unlisted` (422) before dispatch. On `linux` an entry may be
    absent at the start.
  - On `linux` a denied program is found by where its file lies. It is denied under a symbolic
    link, under a second mount of its directory (`/bin/id` for `/usr/bin/id`) and under a hard
    link in the same directory. `substrate-exec-gate` takes the new option `--deny`.
  - A deny entry holds across the mounts of the view: what is mounted beneath a denied
    directory is denied with it, and a program denied in a file system mounted beneath a
    listed directory is denied there. Where such a mount cannot be compared, a start with a
    deny-list is `exec.sandbox-place-unserved` (501) before dispatch: `mount-covered` or
    `mount-root-deleted` beneath a denied or a listed directory, and `file-system-unlisted`
    beneath a denied one, or for a mount of such a type that is an entry of a directory the
    start expands. The helper expands only the directories on the way to a deny entry, so the
    same mount one directory further from the entry refuses nothing. An allow-list alone is
    unchanged.
  - New refusals at admission, on every adapter and before any fact is read:
    `confinement.exec-denied-paths-without-subpaths` (422) for a deny-list beside an empty
    `exec_subpaths`, and `confinement.exec-denied-paths-exceeded` (422) for more than
    `MAX_EXEC_DENIED_PATHS` (64) entries.
  - Limits: a deny entry fails `execve` of a path, so the loader given a denied program runs
    it and `sh` given a denied script runs it. A run that can write a listed directory copies
    a denied program there and runs the copy. A hard link that existed before the start in
    another listed directory runs.
    **In a listed directory a run can write, a deny entry holds for the files that exist at
    the start, under the names they have then.** A listed directory that holds a denied path
    is expanded into one rule per entry, and a directory inside it that holds no denied path
    gets one rule: what the run creates in it executes, and the rule moves with a rename. So
    a file the run creates or moves can run under any name, the denied name included
    (`copied after a rename 0`, `linked 0`, `renamed 0`). A new file in the expanded directory
    itself has no rule (`copied 126`), and a program rebuilt there loses the right. Nothing
    runs that the run could not already run by copying it into a listed directory it can
    write. A deny-list is exact only where the run cannot write.
    A start with a deny-list reads every directory it expands, and ends with status 125 where
    one cannot be read. Its standard error is then one line that names the directory in the
    run's view, `substrate-exec-gate: cannot read the directory "<path>": <cause>`, which a
    consumer may match.
    **Never list a lower directory of an overlay that is also in the view**: a program denied
    in the overlay runs from the layer.
  - On `macos` the entries are Seatbelt deny rules after every allow. They were read and run
    only in the hosted macOS check; how Seatbelt matches a deny entry in another letter case
    on a case-insensitive volume is not verified.

### Changed in 0.24.0

- **Breaking (`substrate-host` public API, design 35):** `FACT_ORDER` changes type from
  `[Fact; 13]` to `[Fact; 14]`; code that names the array type or relies on its length breaks.
  `ConfinementError` gains the variants `ExecDeniedPathsWithoutSubpaths` and
  `ExecDeniedPathsExceeded`, so an exhaustive match gains two arms. `ConfinementProfile` gains
  `exec_denied_paths`, `ExecListProof` gains `denied_paths`, `seatbelt::BuildProof` gains
  `exec_denied_paths` and `substrate_exec_gate::Lists` gains `deny`, so a literal of each that
  names every field gains one.

- The Git change-set observation compares the worktree with the tree of the baseline commit the
  daemon holds (design 21, *What the change-set observation reads*). Its answer differs from the
  earlier reader's in these cases:
  - A tracked path is unchanged only when the hash of its content equals the baseline's object
    id. Every tracked file is hashed in each observation, and an index entry flagged
    assume-unchanged or skip-worktree does not hide a changed file.
  - The index labels a path `added` or `conflicted` and decides nothing else, neither whether a
    path changed nor whether it is listed: a change that is staged while the worktree holds the
    baseline's bytes is not listed, and a path the index no longer records is compared like any
    other tracked path. An index entry that records another repository's commit is honoured
    only at a path where the baseline's tree records one; elsewhere the path and everything
    beneath it are compared and listed.
  - The workspace's attributes and filters are not applied: a path marked `-diff` gets a textual
    patch, and no line-ending conversion precedes the comparison.
  - `core.fileMode` is not applied: a changed executable bit is always reported.
  - The daemon user's own ignore file and a `core.excludesFile` are not applied.
  - Rules beyond the first 1 MiB of an ignore file do not apply.
  - An untracked directory that holds a `.git` entry of its own is neither entered nor listed;
    the earlier reader failed the observation.
  - A side over `max_file_bytes` has `binary` set and no patch text.
  - `current.oid` is present whenever the worktree side was hashed, a tracked file over
    `max_file_bytes` included.
  - A tracked path that is not UTF-8 fails the observation whether or not it changed.
  - One observation hashes at most 1 GiB of tracked files and baseline objects to compare, in
    the byte order of their paths, and beside that the worktree side of each listed item within
    `max_file_bytes` (at most `max_files` × `max_file_bytes` more). What could not be compared
    is reported as changed, never as unchanged, and the answer is marked: a tracked file larger
    than what is left of the 1 GiB is reported `modified` with `binary` set, no text and no
    `current.oid`, the answer is marked `truncated`, and the comparison goes on with the next
    path. A baseline object larger than what is left ends the answer there, marked `truncated`.
  - Directories nested deeper than 100 are not entered and the answer is marked `truncated`.
  - An object store that holds `info/alternates`, a link, an entry that is neither a directory
    nor a regular file, directories nested deeper than eight or more than 262,144 entries is
    refused `workspace.git-repository-failed`.

### Security in 0.24.0

- **A confined run could change what the disk holds of a file without moving any of its stat
  facts**
  ([ADR 0050](https://github.com/beyond10x/substrate/blob/main/adr/0050-the-floor-refuses-the-ioctl-requests-that-exchange-file-blocks.md)). A
  program in a run with a writable mount on `ext4` could call `ioctl(EXT4_IOC_MOVE_EXT)`, the
  request `e4defrag` uses, on two files it may write. It needs no capability. Observed in a run on
  the `linux` adapter: the call returned 0, one file read as zeros from disk, and device, inode,
  size, modification time and change time of both files stayed as they were. Until the page
  cache dropped the file, every reader still got the old content, so a digest, a review or a
  commit made after the run could describe bytes the disk no longer held. Nothing that decides
  by stat facts, as Git's index does, sees such a change.
  - **Affected:** every earlier release with confined execution, `0.23.0` included: the host
    driver's execs and sessions, and the `linux` confinement adapter since `0.12.0`, alone or
    inside `systemd`.
  - **Now refused:** the seccomp program of every confined run and session (exec, pipe session
    and pty session) answers `EXT4_IOC_MOVE_EXT` (`0xC028660F`), `XFS_IOC_SWAPEXT`
    (`0xC0C0586D`) and `F2FS_IOC_SEC_TRIM_FILE` (`0x4018F514`) with `EACCES` before the kernel
    looks at the descriptor, on every file system and whatever the profile asks. It is part of
    the floor: no fact, no profile member and no wire change. Two of the three were read from
    the kernel source (v6.18) and not run, because no XFS and no f2fs file system was at hand:
    `XFS_IOC_SWAPEXT`, which exchanges the whole content of two files and sets no time, and
    `F2FS_IOC_SEC_TRIM_FILE`, which erases the blocks a file keeps mapped and sets no time and
    no size. The cases hold only the filter's answer for those two. Run, each with
    `EXT4_IOC_MOVE_EXT` called on two files of an `ext4` workspace and refused: an exec, a pipe
    session and a pty session of the host driver, and an exec on the `linux` adapter. A program
    that defragments a file or erases its blocks in place inside a run now gets `EACCES`.
  - **Not covered:** the `oci` and `pod` adapters and the namespace driver's exec pods, whose
    runtimes' default seccomp profiles pass all three requests
    (`story:oci-and-pod-refuse-the-ioctls-that-exchange-file-blocks`, a draft); `local` and
    `systemd` around `local`, which install no filter and promise no confinement; host processes,
    which are unconfined by contract; and a request of the same kind that ADR 0050's survey did
    not find.

- **A start could bind the daemon's own private paths into a run as a read-only source**
  (ADR 0010 and ADR 0033, amendments of 2026-11-22). A read-only root and a read-only file name a
  host path in the request, and the daemon checked the path's form, its mount point and the
  fact, not what the path was. A caller allowed to start an exec could therefore name the
  daemon's state directory, the workspace root with every other workspace beneath it, or its
  key material, and read it inside the run. A writable root has been refused for such a path
  since ADR 0038. Read from the source; no run of it was made before the change.
  - **Affected:** every earlier release in which the daemon publishes `exec.read-only-roots` or
    `exec.read-only-files`, `0.23.0` included, where a caller holds less than the daemon's own
    view of the file system: a hosted deployment. On the local socket the caller is the daemon's
    own user.
  - **Now refused:** before dispatch, when a read-only root's host path equals, lies under or
    contains a path the daemon keeps private, or a read-only file's host path equals or lies
    under one: the state directory (the parent of `--state`), the workspace root, the TLS
    private key, the static-bearer file, every secret-slot source, the namespace driver's
    kubeconfig, the API socket and the delegated cgroup root. The answers are the published
    `exec.read-only-root-absent` and `exec.read-only-file-absent` (422), each with a message
    that names no path; no bundle changes.
  - **Not covered:** which other host paths a caller may name. ADR 0010 makes the source
    caller-selected, so a path outside that set which the daemon's user can read is still
    admitted (`story:read-only-mount-sources-are-declared-by-the-operator`, a draft that needs
    an ADR). A consumer that opens the host driver in process is not reached by the daemon's
    admission. A second name for a private object made on the host, by hard link or bind mount,
    is not compared.

- **The Git change-set observation could return content from outside the admitted workspace**
  (design 21, *What the change-set observation reads*). The observation opened the workspace's
  repository with the Git library, which took from the workspace's own repository metadata
  which directory to compare and how to read an entry. A program in a confined run can write
  that metadata, so a later observation of the workspace could return, as a patch, content of
  files the daemon's user can read outside it, another workspace's among them, and content
  over the per-file bound. Shown by cases that were red on the earlier reader.
  - **Affected:** every earlier release that serves the Git change-set observation, `0.23.0`
    included, for a Git workspace a confined run or a Git writable root can write.
  - **Now:** every byte and every size an answer returns comes from a file beneath the
    workspace's root or from an object checked against the baseline commit the daemon holds.
    The workspace's own repository metadata decides neither which directory is read, nor how an
    entry is read, nor whether a path changed, and its index removes no path from the answer.
    `max_file_bytes` bounds the text of each worktree file, link and baseline object a patch is
    rendered from, an ignore file is read to 1 MiB, and one observation hashes at most 1 GiB to
    compare and at most `max_files` × `max_file_bytes` for the items it lists. The entries
    under *Changed* list where an answer differs from before.
  - **Not covered:** the check of the workspace's object store holds while no program runs in
    the workspace. While one runs, an observation gives no guarantee of an answer, and still
    returns no byte and no size that was not checked
    (`story:daemon-holds-the-baseline-objects-of-a-git-workspace`, a draft). The Git library
    reads the index, the baseline's commit and tree objects and the stored file of each object
    whole, each to its apparent size, which a storage limit that counts blocks does not bound
    (`story:git-read-limits-bound-the-work`, a draft). The baseline-file observation is
    unchanged; it returns only the object the daemon-held commit names.

- `url` moves from 2.5.8 to 2.5.9 in `Cargo.lock`, and `form_urlencoded` from 1.2.2 to 1.2.3
  with it, for [RUSTSEC-2026-0592](https://rustsec.org/advisories/RUSTSEC-2026-0592.html): in
  2.5.8 `Url::origin()` on a deeply nested `blob:` URL overflows the stack, which ends the
  process. The host driver's Git transport calls `Url::origin()` on the URL of each transport
  request (`crates/substrate-host/src/git/network.rs`), in a build with the `git` feature.
  Whether a Git server can make that URL a nested one was not established. Affected: every
  earlier release with a Git source, `0.23.0` included.
