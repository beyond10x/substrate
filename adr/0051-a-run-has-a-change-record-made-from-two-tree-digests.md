---
status: proposed
date: 2026-11-21
---

# ADR 0051: a run has a change record made from two tree digests

Decides `story:exec-file-changes-are-recorded`. The design comes before code (invariant 8): this
record is written before any implementation, and no implementation exists yet.

Decided before this record, and recorded here as inputs: the mechanism and the one-writer rule
(session `sdc`, 2026-11-21, `approval-record:sdc-decides-plan-and-design-2026-11-21`), and the
consumer's requirements (session `runbook`, 2026-11-21,
`approval-record:order-of-work-after-0-23-0-2026-11-21`). Everything else below is this record's.

## Context

- **Substrate records a file change only when its own file routes made it**: the events
  `workspace.file-written` and `workspace.file-deleted`
  ([`operations.rs`](../crates/substrate-store/src/operations.rs), `terminal_transition`). A write
  by a process inside a run leaves no record.
- **`GitChangeSet` is per workspace, not per run**, and exists only for a Git workspace: it compares
  the workspace with its materialization commit
  ([`substrate-wire`](../crates/substrate-wire/src/lib.rs), `GitChangeSet`).
- **`RunObservation` and `AuditRecord` carry no file change**
  ([`confinement/mod.rs`](../crates/substrate-host/src/confinement/mod.rs)).
- **The consumer records changes itself today**, with inotify and attribution by time window
  (`sdc/docs/fs-changes.md` in the `sdc` repository, record `sdc.fs-change/1`). A time window
  cannot separate two processes that write in the same window. The layer that starts a process and
  builds its file-system view can attribute exactly, and that layer is the host driver.
- **The unit is the run.** One bound program, or one agent step with every tool call inside it, is
  one run, and the record answers "what did this action change". Which tool call inside a run wrote
  a file stays the consumer's.

Three words, used the same way throughout: a run that changed nothing has an **empty** record; a
path that ends as it started is **not listed**; a run whose changes cannot be attributed has an
**unavailable** record with a named reason. A fourth answer, **not requested**, belongs to a run
whose profile did not ask (decision 10).

## Decision

### 1. The mechanism: two tree digests per covered mount, made from a manifest the driver keeps

That the record is the difference of two digests of each covered mount's tree, and that a mount
has one writer, was decided by session `sdc` and is not reopened. How the two digests are made is
this record's.

- **A manifest per covered mount.** For every entry it holds the path relative to the mount, the
  kind and the **stat facts**: device, inode number, size, modification time, change time (both
  with their nanoseconds) and mode. For a file it also holds the SHA-256 of its bytes, for a
  symlink that of its target, and one mark, *racy* (below). SHA-256 in lowercase hexadecimal is
  the digest the repository already uses for file content (`FileObservation.sha256`,
  [`substrate-guarded`](../crates/substrate-guarded/src/fs.rs)).
- **The driver keeps the manifest between runs**, one per covered directory and `exclude` list
  (decision 7): in memory, and as an owner-private file beside the content store (decision 6). It
  is in no run's view.
- **A walk** lists every directory and takes the stat facts of every entry, one `statx` per entry.
  It opens and reads a file only when the kept manifest has no entry for the path, a stat fact
  differs from the kept one, the kept entry is racy, or the stat facts are not trusted for the
  mount (below). The digest of every other file is the kept one, and that file is not opened.
- **Before a run** that asked for the record can execute anything, the driver walks every covered
  mount. **After the run's process tree is proved gone** it walks them again. The record is the
  difference of the two manifests.
- **The end walk of one run is the kept manifest of the next.** The next run's before walk is
  still made and still takes the stat facts of every entry, so a change between the two runs is
  read again and is in that run's `before_sha256` (decision 3).
- A mount's **tree digest** is the SHA-256 over its manifest in path byte order: kind, path,
  permission bits and content digest of every entry. It covers no timestamp, owner or inode number.
  The record states it for both sides of every covered mount.
- One writer per mount. A run that shared a covered mount with another writer has an unavailable
  record (decision 2). There is no fallback to a time window, and no record is guessed.

**Why a confined run cannot change a file's content and keep its stat facts.** The digest of a
file that was not read is right only if no change of content leaves all six stat facts as they
were. The argument, with what was observed for it on 2026-11-22 by a throwaway program that is not
committed (Linux 6.18.55, `CONFIG_HZ=1000`; `ext4`, and `tmpfs` and `overlay` over `ext4` mounted
in a user namespace of the program's own):

1. **A run changes a covered file only through the kernel**, on the directory its view binds: a
   write, a truncation, a space operation, a store through a shared mapping, or a replacement of
   the entry at the path.
2. **The kernel moves the change time on each of these, and no call sets a change time.** Observed
   on all three file systems, unless the row says otherwise:

   | the process does | the stat facts afterwards |
   |---|---|
   | `pwrite` and `stat`, 20 000 rounds on one file | the change time differed after every write; the smallest step was 772 ns on `ext4`, 471 ns on `tmpfs`, 978 ns on `overlay` |
   | a write, then `futimens` that sets the modification time back to the old value | the modification time is the old one, the change time moved |
   | `fallocate` that punches a hole, or zeroes a range, and keeps the size | the change time moved (`tmpfs` does not support zeroing a range) |
   | unlink, then a new file at the same path with the old size and the old modification time | the change time differs; the inode number was the same on `ext4` and `overlay` and another on `tmpfs` |
   | the first store through a shared mapping, with or without a read of the page before it | the change time moved on `ext4` and `overlay`; **on `tmpfs` it moved only without the read** (limit c) |
   | the first write to a file that only the lower layer of an `overlay` holds | the change time moved; device and inode number stayed |

3. **A run cannot move the clock the kernel stamps with.** Setting it needs `CAP_SYS_TIME` in the
   initial user namespace, which a run does not hold, and a time namespace offsets only the
   monotonic and boot clocks (inferred from `capabilities(7)` and `time_namespaces(7)`; not tried).

**Where the argument does not hold, and what this record does about each.**

- **a. `EXT4_IOC_MOVE_EXT` (observed on `ext4`): a run can defeat the stat facts today.** The
  online-defragmentation call exchanges the blocks of two files the caller may write. It needs no
  capability, and it succeeded inside a user namespace as well. The file named as the donor held
  65 536 bytes of `V` before the call; after it, the file read from disk as 65 536 zero bytes,
  with device, inode number, size, modification time and change time all unchanged, while its
  cached pages still read `V` until they were dropped. **The same call defeats a walk that reads
  every byte**: an end walk reads the cached pages and digests the old content, and the file
  changes when the cache drops them, after the record settled. The floor's seccomp program refuses
  one `ioctl` request today, `TIOCSETD` (`filters` in
  [`seccomp.rs`](../crates/substrate-host/src/seccomp.rs)), and the call reaches the file system
  from a confined run (observed, 2026-11-22): a static program started as one run on the `linux`
  adapter, under bubblewrap, the seccomp program and a delegated cgroup, with a writable
  workspace on `ext4`, made the call and got `0` with 16 blocks moved; the donor then read from
  disk as zero bytes with all five stat facts unchanged. The same run with a read-only workspace
  could not open either file for writing (`EROFS`), so the call needs a mount the run may write.
  A file whose blocks are not yet allocated is refused by `ext4` itself (`ENODATA`), which is no
  protection: a file a walk has read is on disk soon after. **The floor refuses it, by ADR 0050**:
  the operator ordered on 2026-11-22 that the seccomp program of every confined run and session
  answers the request `EACCES`, as it answers `TIOCSETD`, as a story of its own that lands before
  this record's code. This record depends on that decision and adds nothing to the program. No
  shipped path defragments a file from inside a run (inferred).
- **b. A shared mapping that is still held (observed on all three).** After the first store moved
  the change time, a second store through the same mapping moved no stat fact; a store after
  `msync` moved the change time again on `ext4` and `overlay`, and not on `tmpfs`. A process that
  holds a writable mapping while a walk reads the file can change it afterwards, unseen. The
  process has to be alive at the walk. A run's before walk and end walk are made when no process
  of that run exists, and a walk made while a registered writer runs marks what it reads racy by
  the rule below. A process Substrate did not start is decision 3's limit.
- **c. `tmpfs` (observed).** A store through a shared mapping after a read of the page moved no
  stat fact, during the mapping and after it. **The stat facts are not trusted on `tmpfs`.**
- **d. `overlay`.** With both layers on `ext4` every probe gave what `ext4` gave (observed). The
  driver cannot tell the layers' file systems from the mount it walks (inferred), and an upper
  layer on `tmpfs` has limit c. **Not trusted.**
- **e. Every other file system.** A network or user-space file system reports the times its
  server chooses. **Not trusted.**
- **f. A crash, and a clock set back.** After an unclean shutdown a file's data can be newer than
  the inode times the journal kept (inferred), and an operator with `CAP_SYS_TIME` can set the
  clock back so that a later change carries an earlier time. A kept manifest is discarded when it
  was written under another boot identity (`/proc/sys/kernel/random/boot_id`), or when the wall
  clock reads earlier than the manifest's last walk.
- **g. An unconfined process.** A host process (ADR 0036) runs as the daemon's user without the
  seccomp program: it can make the calls of a and b and write the store itself. The argument is
  about confined runs and sessions.

**When the stat facts are trusted.** A walk takes a kept digest for a file it does not read only
when all of these hold. Otherwise it reads every file of that mount, which is the mechanism
without the shortcut:

- the mount's file system is `ext4`: `fstatfs` on the pinned directory answers the magic `0xEF53`;
- every confined writer runs under a seccomp program that refuses `EXT4_IOC_MOVE_EXT` (limit a;
  ADR 0050, decided);
- the manifest was written under the current boot identity, and not later than the wall clock
  now reads;
- the entry is not racy.

**Two changes inside one step of the clock: the racy rule.** A walk that reads a file, and a
change that follows inside the same step of the file system's clock, would leave equal change
times. The rule is the one Git's index applies to the same problem: **an entry whose change time
is not older than the walk's horizon minus `FILE_CHANGE_RACY_WINDOW_MS` is marked racy, and the
next walk reads a racy entry again whatever its stat facts say.** The horizon is the instant the
walk started. For a walk made while a registered writer of the mount runs (the first walk of
decision 8 can be one), it is the instant the oldest such writer was registered: a store through
a mapping moves the change time at its first fault, which is never before its writer's start, so
this closes limit b for every writer Substrate started.

Observed granularity on this kernel, the same on `ext4`, `tmpfs` and `overlay`:

- 4 000 files written in 133 ms with no `stat` between the writes carried 124 distinct change
  times, at a median distance of 999 986 ns (`ext4`; `tmpfs` 31 distinct in 30 ms, `overlay` 135
  in 136 ms). An unobserved write is stamped with the kernel's tick: 1 ms at `CONFIG_HZ=1000`.
- A write that follows a `stat` of the same file took a finer stamp every time: no two change
  times were equal in 20 000 rounds of write and `stat`, nor in 5 000 rounds of `stat`, two writes
  and `stat`. This is what the multigrain timestamps of Linux 6.13 and later do (the attribution
  is inferred; the behaviour is observed): a change after a walk's `statx` cannot repeat the stamp
  that `statx` saw.

The rule does not depend on multigrain timestamps. The window is 2 000 ms because the coarsest
change time a trusted mount can have is one second, on an `ext4` whose inodes have 128 bytes
(inferred from the on-disk format; this host's `ext4` reports nanoseconds), and a tick is at most
10 ms (`CONFIG_HZ=100`, inferred). Its cost: a file changed within 2 s before a walk is read once
more by the next walk.

Not chosen, by session `sdc`: an overlay upper directory per run (exact for two writers, but
concurrent runs stop seeing each other's writes), and fanotify with the writer's pid (inferred: the
marks that report another process's pid need `CAP_SYS_ADMIN`, which the daemon does not hold).
Not chosen, by this record: reading every covered byte at both walks, which was this record's
first draft (measured in decision 13: 8 s to 9 s for every run at 500 MiB, and a bound the
consumer's checkouts exceed); asking `FIEMAP` for every file to see moved blocks (one more call
per entry, for one call that the floor can refuse once); an inotify watch per directory in place
of the `statx` per entry (the queue can overflow, and it reports the writes the change time
already reports).

### 2. Overlap: what counts as a second writer, and the reasons a record is unavailable

**A writer of a mount** is any of these, from the instant the driver registers it until the instant
given:

| writer | until |
|---|---|
| a run or exec whose view binds the mount writable (`start`, `start_for_adapter`, `start_with_git_roots`) | its cgroup is proved empty; for a run that asked for the record, until its end walk has finished or has outlived its bound (decision 8) |
| a session, pipe or pty, whose view binds the mount writable (`start_pipe`, `start_pipe_with_git_roots`) | its cgroup is proved empty |
| a run or session whose process tree could not be proved gone (the driver's `unproven` table) | the tree is proved gone |
| a write through Substrate's own file routes to that workspace (`write_atomic`, `replace_cas`, `edit_cas`, `patch_cas`, `delete_file`) | the call returns |
| a driver operation that writes the directory: a Git materialization into it, a destroy batch | the operation returns |

Not writers: every read route, a run whose view binds the mount read-only, a read-only root
(ADR 0010) over the same directory, and the driver's own walks.

**A host process (ADR 0036) is not started by the driver.** The daemon starts it through
`substrate_host::host_process`, beside the driver, unconfined, with a declared `cwd`. The
in-process path of this story has none. The wire story registers a host process whose `cwd` is a
covered mount's host directory, or beneath it, as a writer of that mount until the process is
gone; a host process elsewhere on the host is outside what the driver sees (decision 3).

**How the driver knows.** It starts every writer of the table. `HostDriver` keeps one in-memory
registry keyed by the directory a mount shows: a workspace's root name, or a declared writable
root's name. A Git checkout mounted as a writable root (ADR 0038 kind 2) has the key of its
workspace, so it overlaps a run that has that workspace at `/workspace`. Two declared roots of
which one lies beneath the other are one key. The unit is the directory: a scoped workspace
access (ADR 0023) does not narrow it, so two runs scoped to different subtrees of one workspace
still overlap.

**The rule.** The record of run *R* is unavailable with reason `second-writer` when any other
writer of any mount *R* covers was registered at any instant between *R*'s own registration, which
precedes its before walk, and the end of its end walk. The rule is symmetric: two recording runs
that overlap both get it, and neither lists a change. Both runs run; nothing is refused or delayed
for it.

**One exception, for runs that follow each other.** A run or session start that would write a mount
on which only another run's end walk is still in progress waits for that walk, at most
`FILE_CHANGE_START_WAIT_MS` (decision 8), and is then no overlap. Without it, the record of every
action that is followed at once by the next action in the same workspace would be unavailable,
which is the consumer's normal use. An end walk takes that long only when the run changed more
bytes than the wait lets it read (decision 13); the start then goes ahead, and it is an overlap:
the bounded start is paid with the earlier run's record. A file-route write that arrives during an
end walk does not wait and is an overlap.

**The reasons an unavailable record carries** are a closed list,
`ChangeRecordUnavailable`. The answer names the reason and, where one mount caused it, the mount:

| reason | when |
|---|---|
| `second-writer` | the rule above. The answer names the other writer so the embedder can remove the cause (below) |
| `outside-writer-detected` | a walk saw an entry change while it read it, and the registry held no other writer (decision 3) |
| `tree-over-bound` | the covered tree holds more entries, bytes or levels than decision 5 allows, at either walk |
| `manifest-not-ready` | the before walk did not finish within `FILE_CHANGE_START_WAIT_MS` on a mount whose stat facts are trusted: its kept manifest is missing, was discarded or is still being made (decision 8). A later run of the same mount can have a record |
| `walk-over-time` | the end walk did not finish within `FILE_CHANGE_WALK_TIMEOUT_MS`; or the before walk did not finish within `FILE_CHANGE_START_WAIT_MS` on a mount where every walk reads every file (decision 1) |
| `paths-over-bound` | the run changed more than `FILE_CHANGE_MAX_PATHS` paths. The answer carries the count of changed paths, so a reader tells 10 001 from 2 000 000 (stakeholder, question 1) |
| `entry-unreadable` | the walk could not open or read an entry (a directory or file without read permission) |
| `mount-crossed` | a directory beneath a covered mount is another file system's mount point |
| `path-unrepresentable` | a changed path is not UTF-8 |
| `content-store-failed` | a side's content could not be kept (decision 6) |
| `run-tree-unproven` | the run's own process tree was not proved gone, so no end walk can be final |

A second writer is known before the end walk starts, so it answers first and no end walk is made.

**What `second-writer` names**, so the embedder can remove the cause (stakeholder, questions 7 and
8), a closed `SecondWriter` beside the reason:

- a run, session, exec or driver operation: its kind and its identity (`RunId`, session id, or the
  operation's name);
- **a write through Substrate's own file routes**: the route and the path it wrote
  (`workspace.file-written` and the rest);
- **a host process (ADR 0036) with its `cwd` in the mount**: its pid and its executable path on the
  host.

It names no changed path and no content: it is enough to point at the writer. The count for
`paths-over-bound` and the `SecondWriter` are the only members an unavailable record carries beyond
the reason and, for `tree-over-bound`, `mount-crossed` and `entry-unreadable`, the mount.

Not chosen: subtracting a file-route write from the record (Substrate knows its path and digests,
but a route write and a run write to one path cannot be ordered from two digests); overlap by
subtree; refusing or delaying the second writer.

### 3. What the host cannot see

**The limit, stated plainly.** A process that Substrate did not start is not in the registry: an
editor, a shell on the host, a file sync, or a second `HostDriver` in another process over the same
workspace root. What such a process writes into a covered mount between a run's two walks is listed
in that run's record as the run's change. The record is exact only for a mount that Substrate's one
driver alone writes while the run runs.

What the driver can detect, and what the record then says:

- **A write that lands while a walk reads.** Each file is `fstat`ed before and after it is read,
  and each directory's entries are opened by name after they are listed. A size, modification time
  or change time that differs across the read, or an entry that is gone when it is opened, while
  the registry holds no other writer, makes the record unavailable with `outside-writer-detected`.
  This catches only a write that coincides with a walk.
- **A change between two runs of one consumer.** The record states each mount's tree digest before
  and after (decision 4). A consumer that runs its actions one after another compares one run's
  `after` digest with the next run's `before` digest: a difference is a change no run of its own
  made. The driver does not make this comparison for the consumer. The before walk sees such a
  change by a stat fact that moved (decision 1), and reads that file again.
- **Not seen: a change that moves no stat fact.** A process Substrate did not start that keeps a
  writable shared mapping of a covered file can store through it after a walk read the file
  (decision 1, limit b). The kept digest of that file is then wrong until one of its stat facts
  moves, and a record that later lists the file states a before side that is older than the
  file was. An unconfined process can also make the block exchange of limit a.

Nothing else is detected. Not chosen: a lock file per mount shared by every driver process
(it would make a second driver visible, and is a story of its own if several processes open drivers
over one root); inotify on the mount (it reports that something changed and not who).

### 4. The record

`ChangeRecord`, read by the run's identity:

| member | meaning |
|---|---|
| `run` | the run's identity (`RunId`; under `linux` the exec identity) |
| `started_at`, `ended_at` | UTC instants the driver observed: immediately before it releases the run's launch barrier, and when it proved the run's cgroup empty |
| `mounts` | one entry per covered mount, in the order `/workspace`, then the writable roots as the start listed them: `mount` (the path in the run's view), `kind` (`workspace`, `host-root`, `git-root`), `name` (root name, declared name or workspace id), `excluded` (the names left out, decision 7), `before_sha256` and `after_sha256` (the tree digests) |
| `changes` | the changed paths, ordered by mount and then by path bytes; one entry per path |

One changed path, `FileChange`:

| member | meaning |
|---|---|
| `mount` | the covered mount the path is in, as `mounts` names it |
| `path` | relative to the mount, UTF-8, `/`-separated |
| `operation` | `create`, `modify`, `delete` or `rename` |
| `from` | for a rename, the path the entry left, in the same mount |
| `before`, `after` | the two sides. `before` is absent for a create, `after` for a delete. For a rename `before` is the side that was at `from` |
| `replaced` | for a rename that landed on a path that already held an entry, the side that was there |
| `fs_mtime` | the after side's modification time as the end walk read it. Absent for a delete |

One side, `ChangeSide`: `kind` (`file`, `symlink`, `directory`, `special`), `sha256` (a file's
bytes, or a symlink's target; absent for a directory and a special file), `size`, `mode` (the
permission bits) and `content` (decision 5).

**A path is listed when its kind, its content digest or its permission bits differ between the two
walks.** A path that the run wrote and restored is not listed. A change of timestamps, owner or
extended attributes alone is not a change.

**`fs_mtime` is the file system's time and never an observed instant.** It costs nothing beyond the
stat facts the walk already takes of every entry (decision 1).
A process inside the run can set a file's modification time (`utimensat`,
`touch -d`, `cp -p`, an archive extraction), so the field orders changes only for writers that do
not. For a rename it is the modification time of the entry at its new path: `rename(2)` does not
change it, so it is the time of the entry's last content write, which can lie before the run's
start, and it does not date the rename.

**Create, modify and delete** follow from the manifests: present only after, present on both sides
and different, present only before. A change of kind at one path (a file replaced by a symlink) is
a `modify` whose two sides state the two kinds.

**Rename.** Two tree digests hold no rename event, so a rename is defined by its effect: the entry
that was at `from` is at `path`, and `from` is gone. Within one mount, a path that is gone and a
path whose after side has the same kind and content digest as the gone path's before side are
paired as one `rename` when either

1. both also have the same inode number, or
2. the pairing by digest is unique: exactly one gone path and exactly one such destination carry
   that digest.

Every other gone path is a `delete`, and every other destination a `create` or `modify`. The
consequences, stated because the consumer sees them:

- **An atomic replace** (a new file written under a temporary name and renamed over the target, as
  `sed -i` and editors do) is a `modify` of the target. The temporary name exists at neither walk
  and is not listed. It cannot be told from an in-place write, and is not.
- **A rename with a content change** (`mv a b`, then an edit of `b`) is a `delete` of `a` and a
  `create` of `b`. The inode number alone is not used: `ext4` reuses a freed inode number at once
  (observed on the workstation: a file created after another was removed got the removed file's
  inode number, in two of two tries), so `rm a; echo x > b` would be reported as a rename.
- **A copy followed by a delete of the source** has the effect of a rename and is reported as one.
- **A rename across two mounts** is a `delete` and a `create`.
- **Several gone paths and destinations with one digest** (empty files, copies) are paired only by
  inode number; the rest are `delete` and `create`.

**Other entries.**

- **Symlinks** are listed like files. The digest is over the target text, read with `readlinkat`
  and never followed; the target is the side's content.
- **Directories** are listed for `create` and `delete` only, with no digest. A directory's
  `modify` is the entries beneath it. A renamed directory is a `delete` and a `create` of the
  directory and a `rename` of each file in it. A change of a directory's permission bits is not
  listed.
- **Mode changes** of a file are a `modify` whose two sides have equal digests and different
  `mode`.
- **Special files** (FIFO, socket, device node) are listed for `create` and `delete` with kind
  `special`, never opened and never digested.
- **Hard links.** Each path is an entry of its own: two paths of one inode whose content changed
  are two `modify` entries. A path linked to an inode that also has a name outside the mount is
  digested like any file; a write through the outside name is an outside writer (decision 3).

Not chosen: one entry per event (two digests hold no events); pairing a rename onto an existing
path as `delete` of the target plus `rename`, as `sdc.fs-change/1` does (one path would be listed
twice; `replaced` carries the same information once).

### 5. Bounds, and what is recorded beyond them

Each bound is a named constant of `substrate-host`.

| constant | value | bounds |
|---|---|---|
| `FILE_CHANGE_MAX_PATHS` | 10 000 | changed paths in one record; more is `paths-over-bound` |
| `FILE_CHANGE_MAX_CONTENT_BYTES` | 1 048 576 (1 MiB) | content kept for one side of one path; equal to `substrate_wire::MAX_IO_BYTES`, so one wire read can carry it |
| `FILE_CHANGE_MAX_RUN_CONTENT_BYTES` | 67 108 864 (64 MiB) | content readable for one record, summed over the kept sides in record order |
| `FILE_CHANGE_BINARY_SNIFF_BYTES` | 8 192 | the prefix searched for a NUL byte |
| `FILE_CHANGE_MAX_TREE_ENTRIES` | 200 000 | entries of all covered mounts of one run together, at any walk: what one walk lists and takes the stat facts of, and what one manifest holds |
| `FILE_CHANGE_MAX_TREE_BYTES` | 8 589 934 592 (8 GiB) | the sizes of the files of all covered mounts of one run together, at any walk: what a first walk reads. **It does not bound what one run changes** |
| `FILE_CHANGE_MAX_DEPTH` | 64 | directory levels beneath a mount; equal to `substrate_wire::MAX_PATH_DEPTH` |
| `FILE_CHANGE_MAX_PATH_BYTES` | 4 095 | bytes of one relative path |
| `FILE_CHANGE_START_WAIT_MS` | 5 000 | how long a start waits for its before walk, a walk already in progress on the mount included (decision 8) |
| `FILE_CHANGE_PREPARE_WAIT_MAX_MS` | 600 000 (10 min) | the largest bounded wait `prepare_file_changes` accepts (decision 11), so a consumer can wait out a first walk of a large cold tree (decision 13) |
| `FILE_CHANGE_WALK_TIMEOUT_MS` | 30 000 | wall time of one end walk. **This is what bounds the bytes one run changes**: the end walk reads them inside it |
| `FILE_CHANGE_RACY_WINDOW_MS` | 2 000 | the racy rule's window (decision 1) |
| `FILE_CHANGE_BACKGROUND_WALKS` | 1 | background walks one driver runs at a time (decision 8) |
| `FILE_CHANGE_MAX_LOADED_ENTRIES` | 1 000 000 | manifest entries one driver holds in memory; a manifest beyond that stays in its file and is read when a walk needs it |
| `FILE_CHANGE_MANIFEST_IDLE_MS` | 86 400 000 (24 h) | how long a manifest no run asked for is kept (decision 6) |
| `FILE_CHANGE_MAX_EXCLUDES` | 16 | names a profile may leave out (decision 7) |
| `FILE_CHANGE_RETENTION_MS` | 3 600 000 (1 h) | how long a settled record and its content are kept |
| `FILE_CHANGE_MAX_RECORDS` | 128 | settled records one driver keeps; equal to the default `HostConfig::max_tracked_execs` |
| `FILE_CHANGE_STORE_MAX_BYTES` | 8 589 934 592 (8 GiB) | the content store, manifests' and records' content together (decision 6); the default of an operator setting |
| `FILE_CHANGE_AUDIT_MAX_BYTES` | 512 | one serialized audit record (decision 12) |

**Content availability** of one side, a closed list:

| `content` | when | digests |
|---|---|---|
| `kept` | a file of at most `FILE_CHANGE_MAX_CONTENT_BYTES` that is not binary, or a symlink | stated |
| `over-bound` | a file over `FILE_CHANGE_MAX_CONTENT_BYTES` | stated; the file is read and digested, not kept |
| `binary` | a file with a NUL byte in its first `FILE_CHANGE_BINARY_SNIFF_BYTES`, or that is not UTF-8 | stated |
| `run-bound` | the record's `FILE_CHANGE_MAX_RUN_CONTENT_BYTES` was spent on earlier sides | stated |
| `not-content` | a directory or a special file | none |

The binary rule and the 1 MiB bound are the consumer's own (`sdc.fs-change/1`: `binary`,
`too_large`), so retiring its recorder changes no classification.

**A covered tree over `FILE_CHANGE_MAX_TREE_ENTRIES`, `FILE_CHANGE_MAX_TREE_BYTES` or
`FILE_CHANGE_MAX_DEPTH` has an unavailable record with `tree-over-bound`, and the run itself
runs.** A walk's first pass counts entries and file sizes from the stat facts and stops at the
first overrun, before any file is read (decision 8); no end walk is made. A tree that grows over
a bound during the run is `tree-over-bound` at the end walk.

**The three time bounds, and the reason each carries.** A before walk over
`FILE_CHANGE_START_WAIT_MS` is `manifest-not-ready` or `walk-over-time` (decision 8). An end walk
over `FILE_CHANGE_WALK_TIMEOUT_MS` is `walk-over-time`. A background walk has no time bound, and
answers for no record. The numbers are justified in decision 13.

Not chosen: a truncated list with a flag for more than `FILE_CHANGE_MAX_PATHS` changes (it would be
a fourth kind of answer beside empty, not listed and unavailable; the remedy for a run that
changes more is an `exclude` name); a byte bound on what one run changes (the end walk's time
bound already bounds it, and a second bound would be a second reason for the same thing).

### 6. The before side: a content store keyed by digest, outside every view

Two digests do not hold the old content. The content of every file that could change must be kept
from before the run until its record is no longer readable.

| candidate | cost | failure mode on `ext4`, `overlay`, `tmpfs` |
|---|---|---|
| a reflink snapshot of the tree (`FICLONE`) | none per byte where it works | `ext4` has no reflink (observed on the workstation: `cp --reflink=always` fails with `Operation not supported`); `overlay` passes the call to its upper file system, so over `ext4` it fails the same way (inferred); `tmpfs` has none (inferred). Unserved on all three listed types |
| a hard-link snapshot of the tree | one `link` per file, no data | on all three, a link shares the inode: a run that writes a file in place changes the kept side. Only a run that replaces files atomically leaves it intact. It also fails with `EXDEV` when the snapshot is on another file system, and on `overlay` a link to a lower-layer file copies it up (inferred) |
| copying every file under a size bound at the start, per run | every eligible byte is written again for every run | none of its own on the three; `ENOSPC`, and on `tmpfs` the copy is charged to memory |
| **a content store keyed by digest** | an eligible byte is written once per distinct content; a walk writes only content it read and the store lacks | none of its own on the three; `ENOSPC`, and on `tmpfs` the store is charged to memory |

**Chosen: the content store.** A hard-link snapshot is rejected because a run could change the
kept before side, and a reflink snapshot because it is unserved on the file systems the repository
lists (`LISTED_FILE_SYSTEMS`, [`place.rs`](../crates/substrate-exec-gate/src/place.rs)). The store
is chosen over a per-run copy because it writes unchanged content once: Loom's concurrent work
trees of one repository share nearly every blob, and with the kept manifest a walk writes only
what changed since the walk before it.

- **Where.** `HostConfig.file_change_root`, an owner-private directory (mode 0700) that defaults
  to `<workspace_root>/.substrate-file-changes`, as `aperture_root` defaults beneath the workspace
  root. Startup refuses, as `config.file-change-root-invalid`, a root that is, lies beneath or
  contains a workspace directory or a declared writable root. The kept manifests (decision 1) are
  files of the same root, mode 0600, each written under a temporary name and renamed into place
  when a walk that changed it has finished.
- **What.** One file per distinct content, named by its SHA-256, mode 0400, written under a
  temporary name and renamed into place. No `fsync`: a blob is checked against its name whenever
  its content is read back, and a mismatch is the named answer `exec.file-change-content-lost`.
- **When.** A walk writes a blob only for a file it read, when decision 5 calls that content
  `kept` and the store holds no blob of that name and size. The first walk of a mount reads every
  file and writes every eligible one. Every later walk writes what changed since the walk before:
  the before walk what something changed between two runs, the end walk the after side of what
  the run changed. One walk never reads a file twice.
- **A run can never change the kept before side.** The store is in no run's view: no mount of any
  run shows it, and the startup rule keeps it outside every directory a run can be given. A blob
  is a file of its own, never a link to a workspace inode, so a write to the workspace file does
  not reach it.
- **How long a blob is kept.** A blob is referenced while a kept manifest names its digest or a
  retained record lists it as a side. **A blob that no manifest and no retained record references
  is removed**, when the walk or the retention step that dropped its last reference finishes.
  There is no cache of unreferenced content.
- **How long a manifest is kept.** Until the first of: the driver destroys the workspace; no run
  has asked for a record of that mount for `FILE_CHANGE_MANIFEST_IDLE_MS`; the store is over its
  budget and this manifest is the one unused the longest among those no running run needs. A
  manifest survives a restart of the driver as its file. It is read back when its boot identity
  is the current one (decision 1, limit f) and the restart cleanup proved that no process of the
  earlier driver remains; otherwise it is removed, and the mount's next walk is a first walk.
- **The budget.** `HostConfig.file_change_store_bytes`, default `FILE_CHANGE_STORE_MAX_BYTES`. A
  walk that would take the store over it first drops idle manifests in the order above. When
  that does not make room, the record is unavailable with `content-store-failed`.
- **Failure.** A blob that cannot be written (`ENOSPC`, `EDQUOT`, a permission error) makes that
  run's record unavailable with `content-store-failed`. The run runs.

**The store's growth is the mechanism's cost in disk.** While a manifest is kept, the store holds
one copy of every distinct `kept`-eligible content of its tree: the text files of at most 1 MiB.

- Measured (decision 13): 413.6 MiB of blobs for the generated tree of 499.5 MiB, 83 % of it.
- Counted on three checkouts of the workstation on 2026-11-22, by file size alone and with `.git`
  left out, so each last column is an upper bound that still counts binary files:

  | checkout | entries, MiB with build output | entries, MiB without `target` and `node_modules` | of that, MiB in files of at most 1 MiB |
  |---|---|---|---|
  | `substrate` | 65 519, 1 786 | 15 668, 65 | 65 |
  | `sdc` | 172 436, 2 448 | 170 174, 1 994 | 1 742 |
  | `loom` | 3 413, 21 | 3 413, 21 | 21 |

- Trees that share content share blobs: ten work trees of one repository cost about one copy.
- After a mount's first walk the store grows by what runs change. A changed eligible file adds
  its new content; its old content goes when the record that lists it is past retention
  (decision 11).

**The store holds copies of workspace files**, a `.env` among them, for as long as a manifest or a
record references them. It is owner-private like the workspace root. A consumer that must not have
a file copied names it in `exclude`.

Not chosen: reading the before side of a tracked file from the workspace's Git object store (the
run can rewrite the object store, and `.git` is not covered); compressing blobs (it would add a
compression dependency for a cost that `exclude` already bounds; question 17).

### 7. Mounts covered, and what is left out

- **Covered: what the run's view binds writable from the host.** The workspace: all of it under
  `WorkspaceAccess::ReadWrite`, each writable subtree under a scoped access, nothing under
  read-only. And every declared writable root of the start (ADR 0038): a named host path, and on
  the wire path a Git checkout. A change in a writable root is listed with that root's mount.
- **`/scratch` is not covered.** It is created empty for the exec and removed when it ends
  (`prepare_scratch` in [`process.rs`](../crates/substrate-host/src/process.rs)), so it has no
  before side and nothing a later action can read. The `linux` adapter requests none.
- **`/tmp` and the sandbox root are not covered.** They are file systems of the run's own mount
  namespace and never exist on the host.
- **`.git` is left out except `hooks/`, `config` and `info/`** (stakeholder, question 4). A
  directory named `.git`, at any depth, is not listed and not descended into, with one exception:
  its children `hooks/` (descended into), `config` (listed) and `info/` (descended into) are
  always covered, and so is each entry beneath `hooks/` and `info/`. Everything else of `.git` —
  the object store, the index, the reflog, `refs/`, `logs/` — stays out. A `git commit` inside a
  run therefore shows the work-tree files it changed and, of `.git`, only these three. **The
  reason** is the stakeholder's: the confined run never commits; a host-side tool commits after it
  and would execute a hook, a `config` directive or an `info/attributes` filter the run planted,
  outside the confinement. The record must show those.

  **Why the fixed rule and not a profile member that includes all of `.git`** (the two forms the
  stakeholder offered as equal): a member that covers the whole of `.git` pulls in `.git/objects`,
  which on a real checkout is tens of thousands of entries and hundreds of MiB — over the tree
  bounds of decision 5, so the record would be `tree-over-bound` on the repositories that need it.
  The fixed rule covers exactly the three attack surfaces at a cost of a few files, needs no new
  profile field to build, validate and bound, and cannot be set to a value that breaks the bounds.
  **What the fixed rule still does not see**: a linked worktree or a submodule, where `.git` is a
  *file* that points at `../.git/worktrees/<name>/` or `.git/modules/<name>/` — there is no
  `.git/hooks` directory to cover, and those hooks live at a path the rule does not name; a
  submodule's `.git/modules/<name>/hooks`; and `core.hooksPath` in `config` pointing at a directory
  outside `.git` (the change to `config` itself is seen, so a reader learns the redirection, but
  the hooks at the new path are covered only if that path lies in a covered mount and no `exclude`
  removes it). Covering a linked or submodule `.git`'s hooks is a story of its own, not a field:
  it means reading the `.git` pointer file, resolving it, and covering a second subtree, with its
  own bounds. The stakeholder's runbooks commit in a plain checkout, so the fixed rule serves them.
- **The profile names what else is left out.** `exclude` holds at most `FILE_CHANGE_MAX_EXCLUDES`
  names. Each is one path component: not empty, not `.` or `..`, without `/` and without a control
  character, at most 255 bytes. An entry with that name, at any depth of any covered mount, is not
  descended into: its contents are not listed. **The directory itself is listed when it is created
  or deleted** (stakeholder, question 3, under question 5's rule for directories), so a new
  `target/` at a new place is visible as a create though its contents are not walked. The
  consumer's recorder leaves out `target` and `node_modules` the same way. **The record's `mounts`
  name the excludes that were applied** to each mount, so a reader sees what was not looked at. The
  tree bounds of decision 5 count only what is covered. A kept manifest belongs to one `exclude`
  list (decision 1): a run that names another list has a manifest of its own and pays that
  manifest's first walk, in which the store already holds the content the two lists share.

Not chosen: reading `.gitignore` (the run controls the file, and its grammar is not the driver's
to parse); paths anchored at the mount in `exclude`; covering `/scratch` as a list of creates; a
profile member that includes the whole of `.git` (the bounds reason above).

### 8. The walk, and what it does with a tree a run controls

The walk runs on the host, on a blocking thread, over the descriptors the start pinned at
admission: the objects bubblewrap binds are the objects walked. It uses the repository's guarded
I/O and nothing else: `openat2` with `RESOLVE_BENEATH`, `RESOLVE_NO_SYMLINKS`,
`RESOLVE_NO_MAGICLINKS` and `RESOLVE_NO_XDEV`, one path component at a time relative to the parent
directory's descriptor, files with `O_NOFOLLOW | O_NONBLOCK`
(`GUARDED_RESOLVE` and `FILE_READ_FLAGS` in
[`substrate-guarded`](../crates/substrate-guarded/src/fs.rs)). An entry's stat facts are taken with
`statx` relative to its parent directory's descriptor, with `AT_SYMLINK_NOFOLLOW`, asking for the
modification and change time by name. **No file is read through a path.** Every read is from a
descriptor whose `fstat` says it is a regular file, and the stat facts kept for a file that was
read are those of that `fstat`.

**A walk has two passes.** The first lists every directory, takes every entry's stat facts,
counts entries and file sizes against the tree bounds (decision 5) and decides which files have to
be read (decision 1). The second reads those files, digests them and writes their eligible content
to the store (decision 6). A tree over a bound is known when the first pass ends, before any file
is read.

**When a walk is made, and what waits for it.**

- **The before walk** is made after every refusal of the start is decided and before any secret
  is read or any process spawned. **The start waits for it at most `FILE_CHANGE_START_WAIT_MS`.**
  A before walk that has not finished by then stops answering for the record, and the run starts.
  Its record is unavailable: with `manifest-not-ready` on a mount whose stat facts are trusted,
  where the manifest will be complete for a later run, and with `walk-over-time` on a mount where
  every walk has to read every file, where waiting again would not help.
- **The end walk** is made after the run's cgroup is proved empty, so no process of the run
  exists while it reads. The run's terminal observation is published before the end walk starts:
  **a walk never holds up a run's end.** It has `FILE_CHANGE_WALK_TIMEOUT_MS`; over that, the
  record is unavailable with `walk-over-time`.
- **A walk that outlived its bound is not thrown away.** It goes on as the mount's background
  walk, so that the manifest is complete for a later run. A background walk can run beside a
  running writer, and then marks what it reads by the racy rule's earlier horizon (decision 1).
  One driver runs `FILE_CHANGE_BACKGROUND_WALKS` background walks at a time; others wait their
  turn.
- **A start that finds a walk of the mount in progress**, the end walk of the run before it or a
  background walk, waits for that walk inside its own `FILE_CHANGE_START_WAIT_MS` and then makes
  its before walk from the manifest that walk left.

**The first walk of a mount reads every file, and no start waits for it.** A mount has no usable
manifest before its first record is asked for, after its manifest was dropped (decision 6), and
after its manifest was discarded (decision 1, limit f). Reading a checkout takes from seconds to
minutes (decision 13). Therefore:

- **The first walk is started by whoever asks first, and runs as a background walk.** That is
  `prepare_file_changes` (decision 11), which an embedder calls when it creates, materializes or
  adopts a workspace it will record in; or else the first run that asks.
- **A run that asks before the first walk has finished starts after at most
  `FILE_CHANGE_START_WAIT_MS`**, and its record is unavailable with `manifest-not-ready`. What the
  walk has read by then stays in the manifest; the walk goes on beside the run.
- A consumer that needs the record of its first action in a workspace calls
  `prepare_file_changes` with a bounded wait (decision 11) and starts that action only when the
  answer is `ready`. **So the normal path never meets `manifest-not-ready`** (stakeholder,
  question 13): that reason is for a run that started without the prepare call, or whose prepare
  wait was too short for a large cold tree.

Not chosen: a first walk at every workspace create, materialize and adopt (a workspace no run
records in would pay the walk and a copy of its text in the store; the wire story can call
`prepare_file_changes` from those routes, decision 14); a start that waits for the first walk
(measured, decision 13: 54.6 s for 500 MiB with cold pages).

| the tree holds | the walk |
|---|---|
| a file whose stat facts are the kept ones | does not open it; the digest is the kept one, under the conditions of decision 1 |
| a file the run replaced by another of the same size and modification time | reads it: its change time moved (decision 1) |
| a symlink | reads its target with `readlinkat` and never follows it; `RESOLVE_NO_SYMLINKS` refuses a link in any component |
| a FIFO, a socket or a device node | reads its kind from the directory entry and its `statx`, and never opens it. An entry that turned into a FIFO between `statx` and open cannot block the open, which is `O_NONBLOCK`, and is then not a regular file by `fstat` and is not read |
| a path longer than `PATH_MAX` | is unaffected in its opens, which take one component each; stops at `FILE_CHANGE_MAX_DEPTH` levels or `FILE_CHANGE_MAX_PATH_BYTES` bytes with `tree-over-bound` |
| a directory with millions of entries | reads entries in batches and counts each against `FILE_CHANGE_MAX_TREE_ENTRIES`; stops at the bound with `tree-over-bound`. The manifest is never larger than the bound allows |
| a file that grows while it is read | reads at most the size its first `fstat` reported and one byte more, compares a second `fstat` with the first, and answers `outside-writer-detected` when they differ. Total bytes read by one walk are bounded by `FILE_CHANGE_MAX_TREE_BYTES`, a sparse file's logical size included |
| a hard link to a file outside the mount | digests it as a file of the mount. It reads nothing the run could not read, and it writes nothing |
| a mount beneath the tree | does not cross it. A mount the run made is in the run's mount namespace and not in the host's, so the walk sees the directory under it (inferred: a run holds no capability to mount, bubblewrap drops them and `--disable-userns` bars a new user namespace). A mount of the host beneath a covered mount is refused by `RESOLVE_NO_XDEV`, and the record is unavailable with `mount-crossed`: the run's view binds the directory recursively (inferred), so the run could write beneath that mount where the walk does not look |
| an entry it may not read (mode 000) | does not change the mode; answers `entry-unreadable` when the entry has to be read |
| a name that is not UTF-8 | keeps it as bytes in the manifest; a changed one answers `path-unrepresentable` |

**Time and size.** A walk checks its deadline between entries and between read chunks of 1 MiB.
The start of a run is delayed by at most `FILE_CHANGE_START_WAIT_MS`, whatever the tree holds. An
overrun of the end walk leaves the run ended as it was. A background walk has no deadline: it is
bounded by the tree bounds, which the first pass enforces before anything is read, and it stops
when the driver closes or the workspace is destroyed. No walk hangs a run.

### 9. The fact and the probe

- **The fact is `file_changes`**, a member of the model's `Fact` type after `output_live`
  (`Fact::FileChanges`, [`spec/confinement`](../spec/confinement/confinement.yaml)).
- **`linux` proves it at open**, and `systemd` around `linux` proves it where `linux` does. The
  proof needs the confinement floor (`exec.argv-only`, `exec.namespaces`), `exec.cgroup-kill` (the
  end of a run is provable), `workspace.guarded-io` (`openat2` is available), a usable
  `file_change_root`, and two throwaway runs:
  1. the probe makes a directory with five files and makes its first walk, which takes the before
     side into the store;
  2. a sandboxed `/bin/sh` creates one file, appends to one, deletes one, renames one, sets one
     changed file's modification time into the past, and tries to read the store by its host path;
  3. the probe makes the end walk and holds only when the record lists those four changes with
     the expected digests, the kept before content of the appended file is its original bytes, and
     the read of the store failed;
  4. a second sandboxed `/bin/sh` overwrites the fifth file in place with other bytes of the same
     length and sets its modification time back to the old value;
  5. the probe walks from the kept manifest and holds only when that file is listed as a `modify`
     and no file the two runs left alone was opened.
- **The fact rests on a floor that ADR 0050 already changed.** The seccomp program every confined
  run and session gets refuses `EXT4_IOC_MOVE_EXT` before the commit that adds the fact
  (decision 1, limit a): ADR 0050 decided it and its story lands first. That story's cases prove
  the refusal, in a forked child and in a run of the delegated lane. The probe of this fact does
  not: `/bin/sh` cannot make the call.
- **Whether a mount's stat facts are trusted is not part of the fact.** It is decided per mount at
  each walk (decision 1) and changes the cost of a record, never whether one can be had.
- **Where the fact is absent on `linux`, the proof says why.** `AdapterFacts.withheld` gains one
  entry for `file_changes` with a cause from a closed list: `floor-absent`, `cgroup-kill-absent`,
  `guarded-io-absent` or `probe-failed`. `confinement.fact-unserved` repeats it, as it does for the
  executable lists (design 35 D27).
- **A profile that asks for the record where the fact is absent is refused
  `confinement.fact-unserved` (501) naming `file_changes`, at admission, with its audit record, and
  nothing runs** (invariant 3). The start decides the profile again against the adapter's own
  proof, as every start does.
- **`local`, `macos`, `oci` and `pod` do not prove it**, and neither does `systemd` around
  `local`. `local` builds no view, so a run writes wherever the user may. `macos` and `oci` have no
  story that proves two tree digests and a kept before side under their mechanism. `pod`'s
  workspace is a volume the adapter's host does not hold.

### 10. The profile member, and the answer for a run that did not ask

- `ConfinementProfile.file_changes: Option<FileChangeRequest>`, default `None`: off.
  `FileChangeRequest { exclude: Vec<String> }`.
- `Some` implies the fact `file_changes` in `required_facts`. A malformed `exclude` is refused at
  admission, `confinement.file-change-request-invalid` (422), before anything runs.
- A profile that lists `Fact::FileChanges` in `facts` and leaves the member `None` requires the
  capability and asks for no record.
- A profile with the member `None` renders and runs as before this record, but for the one
  `ioctl` request its seccomp program now refuses (decision 9): no walk, no manifest, no registry
  entry beyond the writer registration every start now makes, no store write.
- **`not requested` is the adapter API's answer for a run whose profile did not ask.** It is a
  value, not a refusal, and is distinct from an empty record and from an unavailable one.

### 11. The API, and retention

**The host driver**, keyed by the exec's identity:

- `HostDriver::exec_file_changes(id) -> Result<ExecFileChanges, DriverError>` with
  `ExecFileChanges::{NotRequested, Pending, Available(ChangeRecord),
  Unavailable(ChangeRecordUnavailable)}`. It never waits. `Pending` is a run that has not ended or
  whose end walk has not finished.
- `HostDriver::exec_file_change_content(id, mount, path, side) -> Result<Vec<u8>, DriverError>`,
  `side` one of `before`, `after`, `replaced`. It answers `exec.file-change-not-listed` (404) for a
  path or side the record does not list, `exec.file-change-content-not-kept` (422) naming the
  side's `content` value for anything but `kept`, and `exec.file-change-content-lost` (500) when
  the blob is gone or does not match its digest.
- `HostDriver::prepare_file_changes(workspace, writable_roots, exclude, wait_ms) ->
  Result<FileChangePrepared, DriverError>` asks for the manifests of the mounts a run with that
  workspace, those roots and that `exclude` list would cover. It starts the first walk as a
  background walk where a mount has no usable manifest (decision 8). **It waits up to `wait_ms` for
  every mount's first walk to finish**, then answers `ready` when they all have a manifest or
  `walking` when one does not, and for each mount whether its stat facts are trusted (decision 1).
  `wait_ms` of 0 makes it a status call that never blocks; a positive `wait_ms`, bounded by
  `FILE_CHANGE_PREPARE_WAIT_MAX_MS`, is the bounded wait a consumer uses so that its first run
  never meets `manifest-not-ready` (stakeholder, question 13): the consumer calls
  `prepare_file_changes` with a wait, and starts the run only when the answer is `ready`. The call
  is the one way to learn the manifest is ready; the first run does not have to be the thing that
  waits.
- The request reaches the driver as a member of `AdapterStart`, which is `pub(crate)`; no wire
  type changes here.

**The adapter API**, over those two calls:

- `ConfinementAdapter::file_changes(&self, run) -> Result<FileChanges, ConfinementError>` with
  `FileChanges::{NotRequested, Available(ChangeRecord), Unavailable(ChangeRecordUnavailable)}`. It
  returns once the record is settled: it waits for the run's end as `wait` does, and then at most
  `FILE_CHANGE_WALK_TIMEOUT_MS`. The default implementation, for every adapter that does not prove
  the fact, observes the run and answers `NotRequested`.
- `ConfinementAdapter::file_change_content(&self, run, mount, path, side)`.
- `ConfinementAdapter::prepare_file_changes(&self, profile, wait_ms) ->
  Result<FileChangePrepared, ConfinementError>`, for the mounts that profile would cover, with the
  same `wait_ms` as the driver call. The default implementation answers
  `confinement.fact-unserved` naming `file_changes`.

**The record is not a member of `ExecObservation` or of `RunObservation`.** No existing literal of
either changes.

**Retention.** A settled record and its content are kept until the first of:
`FILE_CHANGE_RETENTION_MS` after it settled; `FILE_CHANGE_MAX_RECORDS` newer settled records exist
in the driver; the driver closes. Retention does not follow the driver's tracking of the exec,
because the daemon retires a terminal exec as soon as it has persisted it. Records are held in
memory and do not survive a restart of the driver. **A read after retention answers
`resource.not-found` (404)**, the answer an unknown run gets: the driver keeps no tombstone.

**The embedder must fetch a record within the retention** (stakeholder, question 9). The record is
the driver's for an hour and no longer, and it is lost at a driver restart; a consumer that keeps
file-change history holds its own copy. Loom reads the record at the run's end and copies it into
its own store, so the hour and the restart do not reach its history. An embedder that reads later
than the hour, or after a restart, gets `resource.not-found` and cannot tell it from a run that
never asked: fetch at run end.

### 12. The audit record

Two new `AuditRecord` variants, each its own variant and a member of no other record:

- `FileChangesRecorded { run, profile, adapter, inner, changed_paths, created, modified, deleted,
  renamed }`, written once when a record settles available, on the task that settled it.
  `changed_paths` is the count of listed paths and the four others its split by operation.
- `FileChangesUnavailable { run, profile, adapter, inner, reason }`, written once when a record
  settles unavailable.

**Neither carries a path, a digest or content.** Every member is an identity, an adapter name, a
count or a reason of the closed list, so one serialized record is at most
`FILE_CHANGE_AUDIT_MAX_BYTES` (512 bytes); a test serializes the largest values and holds the
bound. A run that did not ask writes neither.

Not chosen: no record for an unavailable result (an auditor would not see that a requested record
is missing, and why).

### 13. The cost

Measured on 2026-11-22 with a throwaway program that is not committed. It walks as decision 8
states (`openat2` beneath, one `fstatat` per entry, files opened only to be read), digests with
`sha2` 0.11.0, the workspace's own version, keeps a manifest as decision 1 states with the racy
rule at 2 000 ms, and keeps the before side as decision 6 states. Host: Intel Core i9-10900K, 20
logical CPUs (no SHA extension in `/proc/cpuinfo`, so SHA-256 runs without hardware SHA), Linux
6.18.55, `ext4` on NVMe.

**The load.** Every run of the program was one job of the shared build pool and waited for its
slot. During the kept-manifest series the 1-minute load average was 36 to 44 on the 20 CPUs, and
the pool had no free slot, 11 to 13 jobs running and 24 to 33 queued. Every number is therefore an
upper estimate for this host, and none is a best case. The four jobs ran 6 s, 7 s, 38 s and 56 s.

**The kept-manifest walk.** Three trees, each generated once and removed after its series:

| one walk, wall time | 20 420 entries, 6.7 MiB (20 000 files of 100 to 600 bytes) | 204 200 entries, 66.7 MiB (200 000 such files) | 20 420 entries, 499.5 MiB (20 000 files, the mixed tree below) |
|---|---|---|---|
| first walk: reads every file, pages warm | 0.32 s | 2.1 s | 6.8 s |
| nothing changed, three walks | 0.060 s, 0.083 s, 0.097 s | 1.4 s, 1.0 s, 1.6 s | 0.081 s, 0.054 s, 0.078 s |
| per entry, nothing changed | 2.9 to 4.7 µs | 5.0 to 7.9 µs | 2.6 to 4.0 µs |
| nothing changed, the files' data pages dropped | not run | not run | 0.086 s |
| 1 % of the files changed | 0.092 s (200 files, 0.07 MiB) | 1.4 s (2 000 files, 0.67 MiB) | 0.50 s (200 files, 44.6 MiB) |
| the walk after that, which reads the 1 % again (racy) | 0.10 s | 1.3 s | 0.52 s |
| a walk that reads every file, for comparison | 0.21 s (warm) | 3.5 s (warm) | 22.5 s (most pages cold) |
| the manifest as a file: size, write, read back | 1.96 MB, 0.011 s, 0.010 s | 19.6 MB, 0.29 s, 0.40 s | 1.96 MB, 0.059 s, 0.019 s |

In every series the tree digest of the kept-manifest walks equalled that of the walk that read
every file. The manifest file is 98 bytes per entry for these trees, whose relative paths have 22
bytes.

"Data pages dropped" is `POSIX_FADV_DONTNEED` after an `fdatasync` on every file. A walk that reads
no file does not touch them, and the row shows it. **Dentries and inodes could not be dropped**:
that needs root. A walk after a reboot, or after the kernel reclaimed them, also reads the
directories and inode tables from disk, and is not measured.

**Reading every file**, from this record's first series, on the same day under the same kind of
load. Trees: 10 % of the files binary, one file of 2 MiB per 1 000 files, the rest text with
sizes skewed toward small.

| one walk, wall time | 1 000 files, 10.0 MiB | 20 000 files, 499.5 MiB |
|---|---|---|
| digest only, data pages cold | 0.78 s | 18.6 s |
| digest only, warm (two walks) | 0.067 s, 0.067 s | 4.6 s, 3.9 s |
| before side, data pages cold, empty store | 0.45 s | 54.6 s |
| before side, warm, the store holds every blob | 0.083 s | 4.2 s |
| before side, warm, empty store | 0.11 s | 6.6 s |
| content the before side wrote to an empty store | 899 files, 7.1 MiB | 17 980 files, 413.6 MiB |
| not kept: over the per-file bound, binary | 1, 100 | 20, 2 000 |

Reading the 499.5 MiB tree warm took 3.9 s to 6.8 s over both series, 73 to 128 MiB per
second; the 44.6 MiB of changed files were read at about 106 MiB per second.

**What a run that asks pays** on a mount whose stat facts are trusted, once the mount has its
manifest: one before walk before it starts and one end walk after it ends.

- At 20 420 entries: 0.05 s to 0.10 s before the start. The end walk is the same plus the bytes
  the run changed: 0.50 s for 44.6 MiB. The first draft's two walks took 8 s to 9 s on this tree.
- At 204 200 entries: 1.0 s to 1.6 s before the start, and the same again after the end.
- The files a run changed in its last two seconds are read once more by the next walk.
- The size of the files does not enter: 499.5 MiB cost what 6.7 MiB cost.

**What a mount pays once**: the first walk, in the background (decision 8). Measured for
499.5 MiB: 3.9 s to 6.8 s warm, 18.6 s to 22.5 s with cold pages, and 54.6 s with cold pages and
413.6 MiB written to an empty store.

**What a mount whose stat facts are not trusted pays**: a walk that reads every file, before and
after every run, as in the second table.

**The bounds this justifies** (decision 5):

- **`FILE_CHANGE_MAX_TREE_ENTRIES` is 200 000**, the largest number of entries that was measured.
  A walk of a tree at the bound took 1.0 s to 1.6 s on the loaded host. The three checkouts
  counted in decision 6 are under it with their build output: 65 519, 172 436 and 3 413 entries.
- **`FILE_CHANGE_START_WAIT_MS` is 5 000**: three times the slowest measured walk at the entry
  bound. After 1.6 s for the stat facts and 0.4 s to read a manifest that was not in memory, 3 s
  are left to read what changed between two runs, about 300 MiB at the measured rate. On a mount
  whose stat facts are not trusted the whole tree has to be read inside the same 5 s: about
  350 MiB to 640 MiB of warm files at the measured rates, and a larger tree there has no record.
- **`FILE_CHANGE_WALK_TIMEOUT_MS` is 30 000** for the end walk: 1.6 s for the stat facts at the
  entry bound and 28 s to read what the run changed, about 3 GiB at 106 MiB per second. That the
  files a run just wrote are still in the page cache is assumed.
- **`FILE_CHANGE_MAX_TREE_BYTES` is 8 589 934 592 (8 GiB), and it is extrapolated, not
  measured.** It bounds the first walk, which holds up no start. Sixteen times the measured tree,
  linearly in bytes: 64 s to 112 s warm, five to six minutes with cold pages, on the loaded
  host. The largest checkout counted in decision 6 is 2 448 MiB with its build output. A larger
  tree was not generated: the workstation's disk was short.
- **`FILE_CHANGE_STORE_MAX_BYTES` is 8 589 934 592 (8 GiB)**: one tree at the byte bound fits
  even if every file of it is eligible. It is a ceiling, not a reservation; decision 6 states what
  the store holds.
- **`FILE_CHANGE_MAX_LOADED_ENTRIES` is 1 000 000**: five manifests at the entry bound. Their
  size in memory was not measured; as files they are 98 MB at 22-byte paths.

The measurement is recorded in the planning store by the story, not by this record.

### 14. The wire shape: a proposal for `story:exec-file-change-record-on-the-wire`

This section proposes. That story's own design confirms or changes it, and no bundle file is
written here.

- **A successor bundle**, the next free version when that story starts, additive (invariant 6).
- **The request.** An optional closed `file_changes: {"exclude": [...]}` on the exec start, and
  on the session start if a session may ask (default: yes, the driver's start path is the same).
  Absent is not requested.
- **The fact.** `confinement.file-changes`, `const true` and absent when not proved (ADR 0048
  D2 and D3), and the word `file-changes` in `sandbox.requires.facts`. The bounds of decision 5
  are `substrate_wire` constants the schemas bind.
- **Why a fact is absent.** Design input from the stakeholder: the capability snapshot should be
  able to name why a fact is absent. Proposed: an optional snapshot member `facts_withheld`, a
  list of `{fact, cause}` with a closed cause list per fact, built from `AdapterFacts.withheld`
  (decision 9) without `path` and `address`, which name host paths. The snapshot digest binds it.
- **The record.** `GET /v1/execs/{exec_id}/file-changes` under the `exec` scope:
  `{exec, status, started_at, ended_at, mounts, changes, reason, next_cursor}` with `status` one of
  `not-requested`, `pending`, `available`, `unavailable`, and `changes` paged by `limit` (at most
  `MAX_LIST_ITEMS`) and an opaque cursor, because one record lists up to 10 000 paths. `mounts`
  states `mount`, `kind`, `name`, `excluded` and the two tree digests, and never a host path.
- **The content.** `GET /v1/execs/{exec_id}/file-changes/content` with `mount`, `path` and `side`,
  answering the bytes. One side is at most `MAX_IO_BYTES`.
- **An event** in the closed union, `exec.file-changes-settled`, with the exec, the status and the
  count, so a client knows when to read.
- **Refusals.** `confinement.fact-unserved` (501) for a start that asks where the fact is absent,
  recorded as refused before dispatch; `exec.file-change-request-invalid` (422);
  `exec.file-change-not-listed` (404); `exec.file-change-content-not-kept` (422);
  `exec.file-change-content-lost` (500); and `exec.file-changes-expired` (404) for a known exec
  whose record is past retention, which the daemon can tell from an unknown exec because it holds
  the exec's durable record.
- **The first walk.** That story decides which routes call `prepare_file_changes` (decision 11).
  Proposed: an optional member on workspace create, materialize and adopt that names the `exclude`
  list, so that the first walk is made before the first exec; and the answer `ready` or `walking`
  as a member of the workspace resource. `manifest-not-ready` is then a `reason` a client can act
  on: it asks again later.
- **The daemon keeps no second copy.** It reads the host-driver calls of decision 11. Whether
  a record must survive a daemon restart is that story's question; the default is no.

### 15. Tests planned

The collection's `AGENTS.md`, section *Tests stay fast*, binds these: no test over 60 s, the gate's
test step within 120 s, an expensive product made once per binary.

| lane | cases | what keeps it fast |
|---|---|---|
| unit, `substrate-host` (portable, no sandbox) | the manifest difference: create, modify, delete, each rename rule and each case where a rename is not reported, a mode change, a kind change, symlink, directory, special file, hard links, the `.git` and `exclude` rules, every content availability, the per-run content bound, the ordering | pure functions over two manifests built in memory; no tree on disk |
| unit, the walk (portable, no sandbox) | every row of decision 8's table, each bound's overrun with its reason, a blob the store already holds is not written again, a blob that does not match its name is `content-lost`, the store root rules | one small tree per case in a temporary directory, tens of files; the bounds are parameters of the walk, so an overrun case uses a bound of 8 entries, not 200 000 |
| unit, the kept manifest (portable, no sandbox) | a file whose stat facts are the kept ones is not opened (the walk counts its opens); each of the six stat facts, moved alone, makes the walk read the file; the racy rule with the clock passed in: an entry whose change time is inside the window is read again by the next walk with equal stat facts, one outside it is not, and the horizon moves back to a registered writer's start; a file system magic other than `ext4`'s, another boot identity and a clock earlier than the manifest each make the walk read every file; the manifest file read back equals the one written; a blob is removed with its last reference and not before; a before walk over a start wait passed as 0 ms answers `manifest-not-ready` and leaves what it read in the manifest | the clock, the boot identity, the magic and the time bounds are parameters of the walk; trees of tens of files |
| unit, the same step of the clock (portable, on the test's own file system) | 200 rounds of: walk, overwrite one file in place with other bytes of the same length, set its modification time back, walk. Every round lists the file as a `modify`. A second case stores through a shared mapping, unmaps, and walks | no sleep: on a trusted mount the racy rule reads the file, on any other every file is read, and the test asserts the record, not which of the two happened. Under one second |
| unit, the seccomp program | a forked child under the program gets `EACCES` for `ioctl(EXT4_IOC_MOVE_EXT)` on descriptor -1, beside the existing case for `TIOCSETD` | no file system; one fork |
| unit, the registry | each writer row of decision 2, the symmetric rule, the wait during an end walk, nested declared roots | no process is started: registrations are calls |
| unit, audit | each variant serialized at its largest values is at most 512 bytes and holds no path | no I/O |
| ESS, `cargo xtask check-ess` | the model's scenarios against the interpreted reference | the committed suite is read; synthesis runs once in the check, not per test |
| delegated, `ess_confinement_linux` and `ess_confinement_systemd` | the model's `-on-delegated` scenarios: admission with the fact, and each `linux-change-record-*` settle step compared with the adapter's own record | one adapter per test binary in a `OnceLock`; the workspaces for all record cases are made once per binary from one template tree of about 40 files |
| delegated, a new `file_changes` test binary | the story's acceptance (two runs at once in two workspaces, each creating, modifying, deleting and renaming through a tool, a shell script and a formatter); two runs in one workspace; a run and a session in one workspace; a run and a file-route write; a run that restores a path; a writable root; a file over the bound; a workspace over a tree bound set low for the case; not requested; the probe's own runs; a run that sets a file's times; a FIFO and a symlink swapped in by the run; a second run in one workspace whose record is right and whose before walk opened no file the first run left alone; a run that overwrites a file in place to the same length and sets its modification time back; a run started before any walk with the start wait set to 0 for the case (`manifest-not-ready`), and the same run after `prepare_file_changes` answered `ready` | one driver and one template tree per binary; every case copies the template (about 40 files) into its own workspace, so cases run in parallel; each run is one `/bin/sh -c` of a few lines; no case needs a large tree because the tree bounds are passed low |
| portable, `ess_confinement_local`, and the `macos`, `oci` and `pod` lanes | the refusal `confinement.fact-unserved` naming `file_changes` | admission only; nothing runs |
| public surface | `confinement-public-items.txt` regenerated; `consumer_surface` builds a profile with the member | no run |

No test walks a tree of the measured sizes. The cost numbers stay a workstation measurement
recorded in the planning store, as `AGENTS.md` § *The gate* requires of repetition and load.

## Consequences

- `substrate-host` gains the profile member, the three adapter methods, the three host-driver
  calls, the record types, the fact and two audit variants. `FACT_ORDER` becomes `[Fact; 14]` and
  `AuditRecord` gains two variants: both break code that names the array type or matches the enum
  exhaustively, and `CHANGELOG.md` says so.
- Every start through the host driver registers as a writer of the mounts it binds writable. That
  is one table insert and removal per start, whether or not any run asks for a record.
- **The floor does not change here.** ADR 0050 made the seccomp program of every confined run and
  session refuse `EXT4_IOC_MOVE_EXT`, whether or not the run asks for a record, and its story
  carries the `CHANGELOG.md` entry. This record needs that refusal in place and adds no rule to
  the program.
- No wire route, schema, fact or refusal moves and no bundle is cut. `HostDriverKind`,
  `ExecObservation` and every released bundle are unchanged.
- A run that asks pays two walks that take the stat facts of every covered entry and read what
  changed (decision 13). A mount pays one walk that reads everything, once, in the background.
  The store holds a copy of the text of every tree that has a kept manifest (decision 6), and the
  driver holds the manifests: up to `FILE_CHANGE_MAX_LOADED_ENTRIES` entries in memory.
- **A record is exact on fewer mounts at low cost than it is available on.** The stat facts are
  trusted on `ext4` only. On `tmpfs`, `overlay` and everything else a record is still made, by
  reading every file at both walks, and only for a tree that can be read inside the start wait.
- **The model is ahead of the committed projections.** `spec/confinement/` holds the fact, the
  two entities and their scenarios. `spec/confinement-suite.json` and
  `crates/substrate-host/src/generated/confinement.rs` are not regenerated with this record,
  because the regenerated `Fact` enum, the open steps that state `file_changes` for `linux` and
  the pinned lane counts are code the implementation must follow. `cargo xtask check-ess` and
  `cargo xtask check-docs` therefore fail on this model until the implementing story's first
  commit regenerates both, moves `FACT_ORDER`, the lane pins and the site's fact tables, as
  `spec/confinement/README.md` describes for a story that adds a fact.
- The consumer's inotify recorder can retire once the record is on the path it uses.

## What this does not decide

- Diff rendering, line counts and syntax highlighting: the client computes a diff from two kept
  sides.
- Attribution below the run.
- A record of reads.
- A record for two writers of one mount.
- The wire: decision 14 is a proposal.
- A lock that makes a second driver process over one workspace root visible (decision 3).
- **Covering a linked worktree's or a submodule's hooks and config** (decision 7, question 4). The
  fixed `.git` rule covers a plain checkout's `hooks/`, `config` and `info/`; a `.git` that is a
  file pointing at another tree, and `core.hooksPath` pointing outside `.git`, are not followed.
  Following them is a story of its own.
- Trusting the stat facts on `overlay`, on `tmpfs` or on any file system but `ext4`. Each needs its
  own evidence: for `overlay`, a way for the driver to prove both layers' file systems; for
  `tmpfs`, there is none while a store through a mapping moves no time (decision 1, limit c).
- A re-read of files whose stat facts did not move, on a schedule, to bound how long a wrong kept
  digest can last where an unconfined process wrote (decision 3).
- Any other call that changes content and moves no stat fact. One was found and is refused
  (decision 1, limit a). The search was the probes of decision 1 and the calls the last section
  lists as considered; it is not a proof that there is no second one.

## Decided by the stakeholder

The eighteen questions this record's draft left open were put to the stakeholder of
`story:exec-file-changes-are-recorded`, session `runbook` (formerly `sdc`), the consumer whose own
recorder this record replaces, and answered on 2026-11-22
(`approval-record:stakeholder-decides-for-the-operator-2026-11-21`). Every default was taken but
one, and the decisions are written into the record above. Recorded here so a reader sees what was
decided and by whom:

- **1 — more than `FILE_CHANGE_MAX_PATHS` changed paths.** Default: the record is unavailable with
  `paths-over-bound`. Condition: it carries the count of changed paths (decision 2).
- **2 — binary files.** Default: digests only, content not kept (decision 5).
- **3 — `exclude`.** Default: entry names at any depth. Conditions: the record names the excludes
  applied to each mount, and a directory with an excluded name is itself listed when it is created
  or deleted though its contents are not (decision 7).
- **4 — `.git`. The one deviation.** The stakeholder needs changes to `.git/hooks/*`,
  `.git/config` and `.git/info/attributes` in the record, because a host-side tool commits after
  the confined run and would execute them. Of the two forms it offered as equal, this record takes
  the fixed rule — `.git` left out except `hooks/`, `config` and `info/` — over a profile member
  that includes all of `.git`, because the latter pulls `.git/objects` over the tree bounds. The
  choice and its limits are decision 7.
- **5 — directories and special files.** Default: listed for create and delete only (decision 4).
- **6 — a rename onto an existing path.** Default: one `rename` entry with `replaced` (decision 4).
  The stakeholder rewrites its reader to match.
- **7 — a file-route write during a run.** Default: the record is unavailable. Condition: it names
  the route and the path (decision 2).
- **8 — a host process with its `cwd` in the workspace.** Default: a second writer. Condition: the
  record names its pid and executable (decision 2).
- **9 — retention.** Default: 1 h and 128 records, lost at a driver restart. Condition stated: the
  embedder fetches within the hour; Loom copies the record at run end (decision 11).
- **10 — a change time beside the modification time.** Default: not in the record (decision 4; the
  manifest holds it, decision 1).
- **11 — one driver per workspace root.** Default: assumed, the limit stated (decision 3).
- **12 — the floor refuses `EXT4_IOC_MOVE_EXT`.** The operator's, and decided: A, on 2026-11-22
  (`approval-record:operator-orders-the-floor-to-refuse-move-ext-2026-11-22`), as ADR 0050 and a
  story of its own ahead of this record. It is the precondition of decisions 1 and 9, now met by
  that record and no longer open here.
- **13 — the first record in a workspace.** Default: a background first walk, `manifest-not-ready`
  until it is done. Condition: `prepare_file_changes` takes a bounded wait, so the normal path
  never meets `manifest-not-ready` (decisions 8 and 11).
- **14 — the start wait.** Default: 5 s for every run (decision 5).
- **15 — the tree bounds.** Default: 200 000 entries and 8 GiB (decision 5).
- **16 — `overlay` and `tmpfs`.** Default: not trusted; every walk reads every file there
  (decision 1). The stakeholder's workspaces are on `ext4`.
- **17 — the store's disk.** Default: one copy of each text file up to 1 MiB per kept tree, 8 GiB
  ceiling (decision 6).
- **18 — a kept digest made wrong by a process outside Substrate.** Default: a stated limit
  (decision 3), given 12 A closes the one known way from inside a run.

**One thing became a story of its own, not a field** (decision 7): covering the hooks and config of
a linked worktree or a submodule, where `.git` is a file pointing elsewhere, or reached through
`core.hooksPath`. The fixed rule of question 4 covers a plain checkout's `.git/hooks`, `config` and
`info/`; the redirected cases need reading and resolving the `.git` pointer and covering a second
subtree with its own bounds. The stakeholder commits in plain checkouts, so this is noted as a
limit, not built here.

## Inferred, not observed

- That fanotify marks reporting another process's pid need `CAP_SYS_ADMIN` (from the story).
- That `overlay` passes `FICLONE` to its upper file system, that `tmpfs` has no reflink, and that a
  hard link to a lower-layer file on `overlay` copies it up. Only `ext4` was tried, and no
  candidate for the before side was run on `overlay` or `tmpfs`.
- That inode reuse and the missing reflink hold on every `ext4`: each was observed on one
  workstation, in one directory.
- That a run under bubblewrap with `--disable-userns` holds no capability to mount, and that
  bubblewrap's writable bind of a directory is recursive.
- **That `EXT4_IOC_MOVE_EXT` reaches the file system from a session.** It was run as one run on
  the `linux` adapter (decision 1, limit a) and not as a session. A session is spawned through
  the same `command` of `process.rs`, which adds the one seccomp program; only the child's
  channel differs.
- Why the donor file read as zero bytes and not as the other file's bytes, and that every `ext4`
  and every kernel behaves so: one kernel, one file system, two calls.
- That no second call changes content on `ext4` and moves no stat fact. Considered and not
  tried: `EXT4_IOC_SWAP_BOOT` (needs `CAP_SYS_ADMIN`), `FICLONE` and `FIDEDUPERANGE` (`ext4` has
  no reflink), direct I/O, `copy_file_range`, `splice` and `io_uring` writes (each a write).
- That a run cannot set the system clock, and that a time namespace leaves the wall clock alone.
- That the fine stamps observed are the multigrain timestamps of Linux 6.13 and later, that a
  kernel before that has none, that an `ext4` with 128-byte inodes keeps change times in whole
  seconds, and that a tick is at most 10 ms.
- That an unclean shutdown can leave a file's data newer than its inode times.
- That the driver cannot tell an `overlay`'s layers from the mount it walks, and that `overlay`
  over a `tmpfs` upper layer behaves as `tmpfs` does. Only `overlay` over `ext4` was tried.
- That sessions get the same seccomp program as runs, and that the driver's restart cleanup proves
  the earlier driver's process trees gone before it serves a start.
- That the files a run wrote are still in the page cache at its end walk.
- That the daemon retires a terminal exec as soon as it has persisted it: read from
  `acknowledge_exec` in [`lib.rs`](../crates/substrate-host/src/lib.rs), not observed on a running
  daemon.
- That the `linux` adapter itself never retires a tracked exec: no call of `acknowledge` or
  `discard_terminal` was found under `crates/substrate-host/src/confinement/`; not run.
- The cost of a walk after a reboot, with no cached dentry or inode: dropping them needs root.
- The cost of the probe's two throwaway runs at open.
- Every number of decision 13 for 8 GiB, that reading scales linearly in bytes, and the cost of
  a tree of more than 204 200 entries.
- The size of a manifest in memory, and the size of its file for paths longer than 22 bytes.
- That the driver's walk, with `statx` and the guarded I/O, costs what the throwaway program's
  walk cost with `fstatat`.
- The largest text file of each measured tree: computed from the generator, not read from disk.
- The share of binary files in the three counted checkouts: they were counted by size alone.
- The cost on an idle machine: every measurement ran while the build pool was full.
- That the ESS model's `-on-delegated` change-record scenarios can be run by a delegated runner as
  decision 15 plans: no runner knows the four new commands yet.
- That the consumer's embedder opens one driver per workspace root.
- That no document `docs/design/36-limit-hits.md` exists yet: the audit variants here are their own
  and belong to no record that document will define.
- No implementation was written or run: every statement about `substrate-host` beyond the cited
  code is a design statement.
