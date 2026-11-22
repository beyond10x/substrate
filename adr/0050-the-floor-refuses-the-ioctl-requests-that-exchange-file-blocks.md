---
status: accepted
date: 2026-11-22
---

# ADR 0050: the floor refuses the `ioctl` requests that exchange file blocks

Decider: the operator, 2026-11-22
(`approval-record:operator-orders-the-floor-to-refuse-move-ext-2026-11-22`): the floor refuses
`EXT4_IOC_MOVE_EXT` for every confined run, as a story of its own ahead of the change record,
with a design note first and a case that shows the call before and after. Decides
`story:floor-refuses-the-ioctl-that-exchanges-file-blocks`. The design comes before code
(invariant 8): this record changes the confinement floor and no code yet. The operator ordered
the one request; the survey below found one more of the same kind, `XFS_IOC_SWAPEXT`, and
decision 1 names it by the same rule. The operator then ordered the rule for every confined run
and the widening to XFS ("A: refuse both", relayed 2026-11-22). A third request,
`F2FS_IOC_SEC_TRIM_FILE`, was found by the adversary pass on 2026-11-22. This repository's
session added it under the same rule and reports it to the operator session with the pull
request: the operator did not order that row.

## Context

### The finding

**A confined run can change what the disk holds of a file without moving any of its stat
facts, and without any reader noticing until the page cache drops the file.** Observed on
2026-11-22 with one run on the real `linux` adapter (bubblewrap, the driver's seccomp program,
a delegated cgroup scope, a writable workspace on `ext4`) that started a static program. The
program opened two files of the workspace, each 65 536 bytes written and synced, and called
`EXT4_IOC_MOVE_EXT`, the request `e4defrag` uses, for 16 blocks:

```
run exited Some(0); program said: MOVER ioctl(EXT4_IOC_MOVE_EXT) rc=0 errno=0 (ok) moved_len=16
donor before facts [66306, 30472565, 65536, 1927699156903466634, 1927699156903466634]
             digest ffe4bc29…295b4c
donor after  facts [66306, 30472565, 65536, 1927699156903466634, 1927699156903466634]
             digest de2f2560…a9cc31
donor after cold content distinct bytes {0}; stat_facts_unchanged=true content_changed=true
```

The second and third line are wrapped here before `digest`. The five facts are device, inode,
size, modification time and change time. The content was read on the host after `fdatasync` and
`POSIX_FADV_DONTNEED`. What the call did, as observed:

- **The donor, the file the request names in its argument, read as 65 536 zero bytes from
  disk; it had held `V`.** The file the request is made on kept its content. Neither file's
  stat facts moved.
- **Until its pages were dropped, every reader still got the donor's old content from the page
  cache.** A digest, a review or a commit made in that window describes bytes the disk no longer
  holds. The change appears when the cache drops the pages, or at the next start of the host,
  with no write by anyone at that moment.
- **It needs no capability and no ownership.** The file the request is made on must be open
  for reading and writing and the donor for writing; with either open read-only the kernel
  answered `EBADF` (ran). The file system has to be `ext4`: `tmpfs` and `overlay` over `ext4`
  answered `ENOTTY` (ran).
- **A read-only workspace stops it**: the program could not open either file for writing
  (`EROFS`), so the call needs a mount the run may write.
- A range whose blocks are not yet allocated is refused by `ext4` itself with `ENODATA`. That is
  no protection: a file that was written is on disk within seconds.

Not staged: a session (a session is spawned through the same `command` of
[`process.rs`](../crates/substrate-host/src/process.rs), which adds the one seccomp program;
only the child's channel differs), a read-only root, and a scoped workspace. The adversary pass
later ran a pipe session and a pty session under the program (*Proof*).

### Why the floor refuses one request today

The program every confined run gets ([`seccomp.rs`](../crates/substrate-host/src/seccomp.rs),
`filters`) refuses a foreign architecture, every x32 call, `io_uring_setup`, the sockets
[ADR 0039](0039-confined-sockets-are-an-argument-allowlist.md) does not list, and one `ioctl`
request, `TIOCSETD`. Every other `ioctl` request reaches the kernel.

### What it breaks, and what it does not

A run may write what it likes into a mount it may write, zeros included. What this call adds is
that the change is invisible and late: no stat fact moves, and the content every reader sees
stays the old one for as long as the cache holds it.

- **The Git change-set route can answer wrongly.** `GET` of a workspace's changes
  ([design 21](../docs/design/21-connector-authorized-git-sources.md)) is libgit2's
  `diff_tree_to_workdir_with_index` (`changes` in
  [`git.rs`](../crates/substrate-host/src/git.rs)), which takes a file whose stat facts equal
  the index entry as unchanged. Shown with the `git` command line on `ext4`, outside a sandbox:
  after the call and a dropped cache, the tracked file hashed to another blob than `HEAD` holds
  while `git status` and `git diff HEAD` printed nothing. The route itself was not run against
  such a file (inferred from the same index rule).
- **"Substrate runs things and reports what it observed" turns hollow for file content.** The
  digest of a file read (`DigestedFileSlice.sha256`) and the digest a guarded write expects
  (`ExpectedFileState::Sha256`) are right for the bytes read. After such a run they stop
  describing the file at a later moment nobody can name, with no write in between.
- **A workspace is durable state, and its durable content is then not what anything read after
  the run.**
- **Any design that decides by stat facts is defeated**, the change record first: ADR 0051,
  proposed and not yet on `main` when this record was written, is where the call was found.

It does not touch what the floor lists by mechanism. The capsule under `/runtime`
([ADR 0009](0009-execution-capsules-are-verified-read-only-inputs.md)), declared read-only
roots ([ADR 0010](0010-declared-host-roots-are-mounted-read-only.md)) and read-only files
([ADR 0033](0033-a-single-host-file-is-admitted-read-only.md)) are read-only mounts, and the
call needs both files open for writing. Another workspace is not in the run's view. Guarded
I/O, no egress, the cgroup bounds and whole-tree kill are unaffected.

### The survey

Which calls let a process without a capability change a file's content, or exchange content
between files, without moving the change time or with the page cache left stale. "Ran" is on
this host: `ext4`, `tmpfs` and `overlay` over `ext4`, outside a sandbox, and `ext4` and the
run's `tmpfs` `/tmp` inside one confined run on `linux`, with the same answers in both except
where noted. "Read" is the kernel source at `v6.18`.

| call | what it does to content, stat facts and cache | needs | from a confined run today | ran or read | the rule |
|---|---|---|---|---|---|
| `EXT4_IOC_MOVE_EXT` | the donor reads as zeros from disk; no stat fact of either file moves; the cache keeps the old content | `ext4` without `bigalloc` or DAX; the file the request is made on open read and write, the donor open for writing; one file system; no capability | reaches `ext4` and succeeds | ran, inside a run too; `ENOTTY` on `tmpfs` and `overlay` | **refused** |
| `XFS_IOC_SWAPEXT` | exchanges the whole content of two files of equal size; the function sets no time; both files' cached pages are dropped first, so readers see the new content | XFS; both files open read and write; the caller passes the first file's current modification and change time; no capability | the request is not filtered; reachable where a writable mount is on XFS | read: `xfs_ioc_swapext` in `fs/xfs/xfs_ioctl.c`, `xfs_swap_extents` and `xfs_swap_extent_flush` in `fs/xfs/xfs_bmap_util.c`. Ran only where it does not exist: `ENOTTY` on all three | **refused** |
| `EXT4_IOC_SWAP_BOOT` | exchanges a file's blocks with the boot loader inode | `CAP_SYS_ADMIN` in the initial user namespace (`capable`) | the kernel answers `EPERM`: a run holds no capability outside its own user namespace | read: `swap_inode_boot_loader` in `fs/ext4/ioctl.c`. Not run: a success would alter a reserved inode of this host's root file system | not refused: not reachable |
| `XFS_IOC_EXCHANGE_RANGE`, `XFS_IOC_COMMIT_RANGE` | exchange ranges of two files; set modification and change time of both | XFS with the feature; both files open read and write; no capability | not filtered | read: `fs/xfs/xfs_exchrange.c` sets both times unless the descriptor came from open-by-handle, which needs `CAP_SYS_ADMIN`. `ENOTTY` on all three (ran) | not refused: the stat facts move, as for a write |
| `FS_IOC_EXCHANGE_RANGE` | none: `include/uapi/linux/fs.h` at `v6.18` defines no such request | | | read | nothing to refuse |
| `F2FS_IOC_MOVE_RANGE` | moves a range into another file; sets modification and change time of both | f2fs; source open read and write, destination for writing | not filtered | read: `f2fs_move_file_range` in `fs/f2fs/file.c`. `ENOTTY` on all three (ran) | not refused: the stat facts move |
| `F2FS_IOC_SEC_TRIM_FILE` | drops the file's cached pages and zeroes or discards the blocks the file keeps mapped; sets no time and no size | f2fs; a regular file open for writing; no capability and no ownership | the request is not filtered; reachable where a writable mount is on f2fs | read by the adversary pass, 2026-11-22: `f2fs_sec_trim_file` and `f2fs_secure_erase` in `fs/f2fs/file.c`. Not run: no f2fs was at hand | **refused** |
| `F2FS_IOC_START_ATOMIC_WRITE`, then `F2FS_IOC_ABORT_ATOMIC_WRITE` or a close | writes after the start are held apart from the file and dropped by the abort, by the close and at the writer's exit, so the file ends with the content it had | f2fs; the file open for writing by its owner (`inode_owner_or_capable`); the difference lasts only while the process that started it holds the file | not filtered | read by the adversary pass: `f2fs_ioc_start_atomic_write` and `f2fs_abort_atomic_write` in `fs/f2fs/file.c`. Not run | not refused: it needs ownership and a live writer and reverts at exit, the class of the shared mapping below |
| `BTRFS_IOC_DEFRAG`, `BTRFS_IOC_DEFRAG_RANGE`, `F2FS_IOC_DEFRAGMENT`, `OCFS2_IOC_MOVE_EXT` | rewrite or move one file's own blocks; the content stays | write access to the file | not filtered | read: `fs/btrfs/ioctl.c`, `fs/ocfs2/move_extents.c`; not run | not refused: the content does not change |
| `FICLONE`, `FICLONERANGE` (`BTRFS_IOC_CLONE`) | the destination takes the source's content; sets its modification and change time | a file system with reflink; destination open for writing | not filtered | `EOPNOTSUPP` on all three (ran). Read: `file_modified` in `fs/remap_range.c` | not refused: the stat facts move, and `cp --reflink` needs it |
| `FIDEDUPERANGE` | shares blocks of two ranges only when their bytes are equal | a file system with reflink | not filtered | `EOPNOTSUPP` on `ext4` and `tmpfs`, `EINVAL` on `overlay` (ran). Read: the comparison in `fs/remap_range.c` | not refused: the content does not change |
| `copy_file_range`, `splice` into a file, a write on `O_DIRECT`, `fallocate` punching a hole | change the destination; modification and change time moved; the cached and the cold content agreed | write access | allowed | ran, all three and inside a run | not refused: a write |
| `renameat2` with `RENAME_EXCHANGE` | exchanges two names; the inode number of each path moved | write access to the directories | allowed | ran | not refused: a stat fact moves |
| `io_uring` writes | | | `io_uring_setup` answered `EACCES` inside the run (ran); outside it succeeded | ran | already refused, by syscall, in `filters` |
| a store through a shared mapping | the content changes and every reader sees it at once; the stat facts lag: on `tmpfs` only the first store through a mapping moved them, and on `ext4` and `overlay` a store moved them again once the page had been written back | write access; a process that holds the mapping | allowed | ran, all three and `tmpfs` inside a run | not an `ioctl`; another class, see below |

**A shared mapping is another class.** There the content a reader gets is the content there is:
the cached and the cold read agreed in every case. What lags is the stat facts, and only while a
process holds the mapping. That is a limit of deciding by stat facts, which ADR 0051 answers
with its own rule for files a live writer may hold; it is not a way to make the disk differ
from what readers see, and no request number names it. Linkers and databases write through
shared mappings, so it cannot be refused.

## Decision

1. **The seccomp program of every confined run and session refuses three more `ioctl` requests
   with `EACCES`**, the errno the profile already answers for `TIOCSETD`:

   | request | number, on `x86_64` and `aarch64` | definition |
   |---|---|---|
   | `EXT4_IOC_MOVE_EXT` | `0xC028660F` | `_IOWR('f', 15, struct move_extent)`, 40 bytes |
   | `XFS_IOC_SWAPEXT` | `0xC0C0586D` | `_IOWR('X', 109, struct xfs_swapext)`, 192 bytes |
   | `F2FS_IOC_SEC_TRIM_FILE` | `0x4018F514` | `_IOW(0xf5, 20, struct f2fs_sectrim_range)`, 24 bytes |

   The rule for the list: **a request is refused when a process without a capability can use it
   to change the content of a file, or exchange content between files, with no stat fact of the
   changed file moving.** `EXT4_IOC_MOVE_EXT` is the one observed. `XFS_IOC_SWAPEXT` is named
   from the source alone: no XFS was at hand, and it is the worse of the two, because the caller
   chooses the content the file ends with. It is the request `xfs_fsr` uses, and no program a
   run is expected to start defragments a file system. `F2FS_IOC_SEC_TRIM_FILE` is named from
   the source alone too: no f2fs was at hand. It exchanges nothing; it erases the blocks a file
   keeps mapped, with no stat fact moving, which is the rule's first half. No program a run is
   expected to start erases a file's blocks in place.
2. **The refusal is the floor, for every run, whatever its profile asks.** It is not a fact and
   no profile member turns it on or off. `Fact` and `AdapterFacts` do not change. The refusal
   answers before the kernel looks at the descriptor, so it holds on every file system and for
   every mount of the run.
3. **The filter matches the request's low 32 bits, as the existing check does.** The kernel
   takes the request as an `unsigned int`, so the low word is the whole request and a value with
   high bits set is the same request and is refused too. A request number holds the direction,
   the size of its argument, a type byte and a number. Both supported targets use the generic
   encoding, and each structure has the same size on both, so one number per request serves
   both (the first two compiled as static assertions for both targets with `clang`, not run on
   `aarch64`; `struct f2fs_sectrim_range` is three 64-bit members). A
   32-bit caller is refused earlier: the program refuses a foreign architecture and every x32
   call before any argument check (ADR 0039 decision 4).
4. **The requests are refused by number; `ioctl` does not become an allow-list.** An allow-list
   would have to name every request an ordinary program in a run makes, and that set is open:
   the terminal requests of a pty session and of every shell and editor
   ([ADR 0019](0019-pty-is-a-second-session-mode.md)), `FIONREAD`, `FIONBIO`, `FIOCLEX`, the
   socket requests, `FICLONE` for `cp --reflink`, `FS_IOC_FIEMAP` and `FS_IOC_GETFLAGS`, the
   pty requests of a program that opens its own terminal, the requests of `perf`, of
   `userfaultfd`, of namespace and pidfd descriptors. The kernel defines requests per driver and
   per file system and adds some in every release. A request missing from the list fails with
   `EACCES` inside some tool, where it reads as that tool's defect. ADR 0039 could list socket
   families because the kernel numbers them in one table and all 46 were tried; `ioctl` has no
   such table. The price of refusing by number is stated in *What stays possible*: a request
   this record does not know passes.
5. **Requests that move a stat fact, need a capability a run does not hold, or leave the
   content as it was are not refused**, each for the reason its survey row gives. `FICLONE` and
   `FICLONERANGE` are the ones ordinary programs need; they set the destination's times, so a
   reader that goes by stat facts sees them as it sees a write.
6. **The refused requests are one list in `seccomp.rs`, and the program's `ioctl` check is
   generated from it**, as the socket chain is generated from `ALLOWED_SOCKETS`: `TIOCSETD` and
   the three requests of decision 1, each with its name and the reason it is there. A request is
   added by adding a row and its case. The three numbers are spelled out with their definition,
   because the `libc` crate names none of them.
7. **No wire change.** No route, schema, fact or refusal of the wire contract moves and no
   bundle is cut. A program that made one of these calls inside a run got its result before and
   gets `EACCES` now; `CHANGELOG.md` records that under *Security*, and that every release with
   the `linux` adapter before this one lets `EXT4_IOC_MOVE_EXT` through.

### The other adapters

| adapter | what a run gets | this record |
|---|---|---|
| `linux` | the program of decision 1 | the refusal |
| `systemd` around `linux` | `linux` starts the run inside the scope | the same refusal |
| `systemd` around `local`, `local` | no seccomp program; the run is declared insecure and has the embedder's uid | nothing: the run can make the call. `local` promises no confinement |
| host processes ([ADR 0036](0036-host-processes-are-a-supervised-resource-family.md)) | `confinement: none` by contract | nothing |
| the host driver inside a pod ([ADR 0046](0046-kubernetes-serves-one-node-bound-daemon-per-stable-pod-address.md)) | the `linux` adapter, so the program of decision 1 | the refusal |
| `oci` | the runtime's default seccomp profile, which `open` requires and Substrate does not change | **nothing: the request passes.** Docker's default profile (`seccomp/default.json` of `moby/profiles`, and `profiles/seccomp/default.json` of `moby` 27.3.1) and containerd's (`contrib/seccomp/seccomp_default.go`, 2.0.0) allow `ioctl` with no argument rule (read). Whether the call then succeeds is the file system of the mounted workspace's |
| `pod` and the namespace driver's exec pods ([ADR 0047](0047-a-kubernetes-namespace-driver-serves-workspaces-on-pvcs-and-execs-as-pods.md)) | `seccompProfile: RuntimeDefault`, the node runtime's default | **nothing: the request passes** (read). A workspace there is a volume, and `ext4` and XFS are what volumes commonly carry |
| `macos` | `sandbox-exec`; no seccomp, and neither request exists | nothing. Not checked: whether APFS has a call that changes a file's content and leaves its change time |

What Substrate could do for `oci` is pass a seccomp profile of its own: that replaces the
runtime's default whole, so Substrate would carry a copy of each runtime's default with the rule
added, and keep it current. For `pod` a `Localhost` profile has to lie on every node, which the
namespace driver, bound to one namespace, cannot do. **Neither is decided here.** These adapters
do not serve the change record (ADR 0051 decision 9), so nothing of Substrate's relies on the
refusal there yet; an adapter that comes to rely on it needs its own decision, and then a fact a
profile can require.

## Proof

| lane | case | what it holds | time |
|---|---|---|---|
| delegated | [`floor_refuses_block_exchange`](../crates/substrate-host/tests/floor_refuses_block_exchange.rs), required by [`scripts/delegated-lane.sh`](../scripts/delegated-lane.sh) | one run on `linux` with a writable workspace on `ext4` starts a static program that calls `EXT4_IOC_MOVE_EXT` on two workspace files; the call answers `EACCES`, and both files keep their five stat facts and their content, read on the host after the pages were dropped. Red before the change: the call answers 0 and the donor reads as zeros | 0.4 s measured, red, release build; one run |
| unit, `seccomp.rs` (portable: a forked child under the program, no sandbox) | beside `the_line_discipline_ioctl_is_refused_and_other_terminal_ioctls_are_not` | on descriptor -1, each request of decision 1 answers `EACCES`, and so does `EXT4_IOC_MOVE_EXT` with high bits set above the low word; `FS_IOC_FIEMAP` (`0xC020660B`, the same type byte), a request with `EXT4_IOC_MOVE_EXT`'s type and number and another size, `FICLONE` and `TIOCGWINSZ` reach the kernel and answer `EBADF` | under 1 s; one fork per probe |
| unit, `seccomp.rs` | the list | every row of the refused list has a name, a number and a reason, and the program refuses exactly the rows | no fork |
| delegated scope | [`adversary_floor_block_exchange`](../crates/substrate-host/tests/adversary_floor_block_exchange.rs), four cases and their probe entry | an exec, a pipe session and a pty session each make the call on two allocated workspace files through every entry a process has to `ioctl` (native, the high word set, x32, the 32-bit entry) and call `XFS_IOC_SWAPEXT`, `io_uring_setup` and a `ptrace` that would suspend the program: each answers `EACCES` or fails, and both files are unchanged, read cold. One more exec asks `F2FS_IOC_SEC_TRIM_FILE` on descriptor -1 and is answered `EACCES`; red before the fourth row, where the kernel answered `EBADF`. `scripts/delegated-lane.sh` runs the root with the host crate's cases and does not require its lines | under 1 s; one run per case |

The lane case is absent, and says why, without a delegated cgroup root, without bubblewrap,
without a `cc` that links a static program, or where cargo's target directory is not on `ext4`:
on another file system the request does not exist and the case would show nothing. The lane
requires its `executed` line. It takes one absence instead and names it as absent, never as
passed: the case's own line that the staging file system is not ext4, which a workstation's
target directory may not be. Every other absence fails the lane. On a hosted runner, where
`GITHUB_ACTIONS` is `true` and the lane records a release, that one absence is not taken either:
the `executed` line is required. There is no case on XFS or on f2fs: no lane has either file
system, and the unit cases and the adversary's cases hold only the program's answer for
`XFS_IOC_SWAPEXT` and `F2FS_IOC_SEC_TRIM_FILE`.

## What stays possible

- **A process outside the floor**: the host's own users and services, a host process of ADR
  0036, a run on `local`, and a run on `oci` or `pod`. Each can make the call on a file it may
  write.
- **A request this record does not name.** The survey read `ext4`, XFS, f2fs, btrfs and ocfs2 at
  one kernel version and ran three file systems. Another file system, an out-of-tree one, or a
  later kernel can have a request of the same kind, and it passes until the list gains it.
- **A file system the survey did not reach** can also give `XFS_IOC_EXCHANGE_RANGE`-like
  requests a different time behaviour than the source read here.
- **Stat facts that lag behind a shared mapping** (the survey's last row).
- A program inside a run that wants to defragment a file gets `EACCES`.

## Consequences

- The floor of `AGENTS.md` § *Safety envelope* gains the refusal, in the story's code change.
- ADR 0039 decided the program's socket rules and its one refused `ioctl` request. This record
  adds three and makes the refused requests a list.
- ADR 0051's precondition, that every confined writer runs under a program that refuses
  `EXT4_IOC_MOVE_EXT`, is met by this record.
- The lane case joins the delegated lane and is red until the program changes. The gate's
  portable lane is unaffected: without a delegated root the case reports itself absent.
- Nothing changes for `oci`, `pod`, `macos` and `local`; the table above says what a run can do
  there.

## Inferred, not observed

- **That `XFS_IOC_SWAPEXT` works as read**: that it needs no capability, sets no time and
  exchanges the content of two files a run may write. Read from `v6.18`; never run, on any XFS.
- **That `F2FS_IOC_SEC_TRIM_FILE` works as read**: that it needs only a descriptor open for
  writing, erases the blocks the file keeps mapped and sets no time and no size. Read from
  `v6.18` by the adversary pass; never run, on any f2fs. The same for
  `F2FS_IOC_START_ATOMIC_WRITE`: that it needs ownership and that every way out of it leaves
  the file's content as it was.
- That a session gets the program through the same `command` as a run is read; that a pipe
  session and a pty session are refused the call is run, by the adversary's cases.
- That a read-only root, a read-only file, a capsule and the read-only part of a scoped
  workspace stop the call as a read-only workspace did: the same mount flag, one of them run.
- That the Git change-set route misses the file as the `git` command line did: libgit2 was not
  run against such a file.
- Why the donor read as zeros and not as the other file's bytes, and that every `ext4` and every
  kernel does the same: one kernel (6.18), one file system.
- That `EXT4_IOC_SWAP_BOOT` answers `EPERM` inside a run: read, not run.
- That the three numbers are right on `aarch64`: the structures' sizes were asserted by a compiler
  for that target; no call was made there.
- That Docker's and containerd's default profiles let the request through at run time, and what
  other runtimes' defaults do: the profiles were read at the versions named; no container was
  started.
- That exchanging blocks between files of two quota projects moves no charge.
- Which requests the programs of a run make: the list in decision 4 is from knowledge of those
  programs, not from a trace.
