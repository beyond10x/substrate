---
format: aep.planning-md/3
id: review-result:mount-fix-security
kind: review-result
status: active
title: 'Security review, three rounds: the mount fix of the allow-list (design 35 D30)'
tags:
- security-review
relations:
- reviews: story:linux-executable-allow-list
revision: 1
---
# Security review of the mount fix (design 35 D30), three rounds

Reviewer: the `security-reviewer` role of the `aep:implementing` skill. Date: 2026-11-21, all
three rounds. The same reviewer session ran all three, after four earlier looks at stories 1
and 2 of design 35, so no round is independent of the others. A round is a review, not an
approval. The reviewer edited no implementation file and made no commit.

Branch `fix/lies-in-follows-the-views-mounts`, from `main` at `df7b970e2`, merged as pull
request 244 (`731d06685`).

| round | commit reviewed | verdict | cases executed | red |
|---|---|---|---|---|
| 1 | `f402165ad` | NEEDS-CHANGE | 718 → 720 | 1 |
| 2 | `d36436a0f` | NEEDS-CHANGE | 748 → 755 | 1 |
| 3 | `ec87d1554` | CONFIRMED | 776 → 778 | 0 |

The finding ids `S5-1` to `S7-2` are assigned in this record. The reports of the rounds
carried no ids. `S5` is round 1, `S6` round 2, `S7` round 3, after the reviewer's fifth, sixth
and seventh look.

How every round ran: each test binary once, `--nocapture --test-threads=1`, in a delegated
cgroup v2 scope (`systemd-run --user -p Delegate=yes --scope`), with the statically linked
helper built from the commit under review. Every mount was made in a private user and mount
namespace. Host: one Manjaro workstation, kernel 6.18, root file system ext4.

## Round 1, `f402165ad`

Verdict NEEDS-CHANGE.

### Findings

- **S5-1, warning, origin pre-existing.** A mount of a file-system type outside the closed
  list was skipped at all four sites that ask whether something lies where a run can write.
  One such file system mounted beneath a workspace and again beneath a read-only root's host
  directory did not overlap, the root was classed `follow`, and a symbolic link a run planted
  on the mount was followed: the walk placed a path only where it ended.
  - Measured: `security5_a_link_on_a_mount_the_rule_skips_is_not_followed_out_of_a_read_only_root`,
    red at `crates/substrate-host/tests/security5_view_mounts.rs:383`. On a `ramfs` the
    listed start answered `ran "0\n"`, a program of `/usr/bin` ran. The same shape on a tmpfs
    was refused `exec.sandbox-path-unrepresentable`. Both controls were green on both types.
  - Origin: by reading, `df7b970e2` follows the link too. Not run at `df7b970e2`.
  - Fixed in `d36436a0f`, both proposed corrections taken. Measured there: the same case
    green, the `ramfs` answer `exec.sandbox-path-unrepresentable`.
- **S5-2, note, origin introduced.** The helper crate gained more public items than the two
  the change named: `Asked`, `Unestablished::place`, `Unlisted`, `lies_in_any`, `overlap`,
  `reported_at`, `MountTree` with `table` and `reaches`, `MountTable::into_tree`,
  `into_tree_reported_by`, `into_tree_as_written`. Nothing outside the crate used
  `reported_at`, `into_tree_reported_by` or `Unestablished::place`.
  - Measured: by grep over both crates and their tests.
  - Reduced in `d36436a0f`: read in its diff, `lies_in_any`, `overlap`,
    `into_tree_reported_by` and `reported_at` lost `pub`, `Unestablished::place` became
    `pub(crate)`.

### Checked and found held

- A mount of a listed type beneath a writable location lies in it, at the four sites
  (`check_location`, `class_of`, `shown_location` in the driver; `open_follow` in the helper):
  executed by the committed cases and by the tmpfs twin of S5-1.
- A writable location on a mount beneath a read-only root: executed, committed case.
- Nested chains and a bind of a subdirectory: by reading, and the committed bind stage.
- Attachment points are compared name by name, no string prefix: by reading.
- Fail closed: a stack of two mounts beneath a workspace root refuses the open
  `config.exec-gate-unserved` with `mount-covered`, executed
  (`security5_ordinary_mounts_do_not_refuse_and_a_stack_beneath_a_writable_location_does`).
- The helper's file on a `ramfs` beneath a workspace: the adapter opened with
  `exec_subpaths proved: false`. On a tmpfs there: `config.exec-gate-invalid`.
- Nothing ordinary broke: a tmpfs beneath `/usr`, a stack no location reaches, a listed
  directory with a mount beneath it, an unlisted start. Executed.
- The host crate's public items: `confinement_public_items` green, its list not in the diff.
- The lane's two branches: the probe exits 0 on this host; inside
  `bwrap --dev-bind / / --unshare-user --disable-userns` it exits 1 and each staged root
  printed its own `ABSENT` line and no `executed` line.

Suite: sixteen binaries, 0 `ABSENT` lines, 1 failed (S5-1).

## Round 2, `d36436a0f`

Verdict NEEDS-CHANGE.

### Findings

- **S6-1, warning, origin introduced.** The helper's name-by-name walk asked the kernel for
  `..` from a directory descriptor in a second system call. A second run that shares a
  writable mount renamed that directory between the two calls, and the walk of a trusted
  link whose target passes through the mount and leaves it with `..` ended one level off.
  The execute rule landed on a directory the entry does not name and the driver, which
  resolved `..` on the text, did not judge. The end was still placed, so it was never in a
  writable mount.
  - Measured, deterministic: `security6_a_concurrent_writer_cannot_move_the_end_of_a_trusted_link_during_the_walk`,
    red at `crates/substrate-exec-gate/tests/security6_gate.rs:486`. `strace` held the helper
    at the one `openat` whose directory was `fs/w/a/d`; with the directory moved the run
    printed `126\n0\n`, the control `0\n126\n`.
  - Measured, soak, local evidence and no test: 4000 starts under a concurrent renamer,
    363 moved. The same binary with the kernel's own walk (no `--writable`): 0 of 4000.
    A directory swapped for a link at the end: 0 unsafe of 4000.
  - Reach: the driver admitted the shape through the real adapter, `ran "0\n126\n"`.
  - Origin "introduced" rests on the 0 of 4000 with the kernel's walk. Inferred, not
    measured: the kernel's walk has the same window in principle, far narrower.
  - Fixed in `ec87d1554`. Measured there: the old shape 0 moved of 4000 (2199 refused
    `passes through`, 1801 did not open), the red case green.
- **S6-2, note, origin introduced.** The design's amendment said
  `MountTree::reaches_as_written` is used for the helper's file too. The code used it only
  for a root's or base directory's source; the helper's file is one place compared with each
  writable location's fail-closed reach, the safer of the two.
  - Measured: by grep, one production caller.
  - Fixed in `ec87d1554`: read there, the sentence says "source only" and states the
    helper's file separately.
- **S6-3, note, origin pre-existing.** `Place::relation`, `Place::lies_in`, `Relation`,
  `MountTable::lies_in` and the free `place::place` had no user outside `place.rs`.
  `Reach::places` and `MountTable::into_tree_as_written` were used by tests only.
  `Unlisted::Refused` had no production user on the branch.
  - Fixed in `ec87d1554`: read there, the first three are `pub(crate)`, the next two
    `#[cfg(test)] pub(crate)`, the two test-only items `#[doc(hidden)]`, and
    `Unlisted::Refused` carries a comment naming story 2 as its caller.
- **S6-4, note, origin introduced.** The lane took the reason `the stage failed` on any host
  for the overlay limit case, and every failing stage command printed it. It could not stand
  in for a failure of the helper, because the stage ends before the helper starts.
  - Fixed in `ec87d1554`: read there, the stage says when the overlay mount itself was
    refused and the lane takes only that reason.

### Checked and found held

- Every symbolic link a followed path reads is placed, in the driver (`judge_link`) and in
  the helper, for `--allow`, `--program` and `--loader`; an unplaced link refuses
  `exec.sandbox-place-unserved` and the helper exits 125; a link in a writable place is
  refused in the helper. Executed: three green helper cases and the `ramfs` case.
- An earlier run cannot move the end of a trusted link through its mount: a directory moved
  away does not open, a link left in its place is refused. Executed.
- A listed start always carries `--writable`: by reading, and the committed argv unit case.
- `Reach::overlaps` is an OR and can only add overlap: by reading. Executed: the
  `security5` `ramfs` twin answered `exec.sandbox-path-unrepresentable`; with no second row
  to compare (`ramfs` beneath the root only) the same link answered
  `exec.sandbox-place-unserved` with `file-system-unlisted ramfs`.
- The helper-file check is monotone in what a writable location reaches, and a helper with
  no place is withheld or refused, never used: by reading, and the two answers of round 1
  measured again.
- A read-only root at `/var/lib` with 6 of 10 mounts beneath it not openable by the user
  starts listed, with the root itself listed, and unlisted. Executed.
- The lane's `require_staged`, cut from the script and run on canned logs: neither executed
  nor absent exits 1; a plain absent name is taken only where no mount is granted; a reasoned
  name is taken only for its own reason; a failing test binary exits 1.

Suite: 755 cases, 1 failed (S6-1), 0 `ABSENT` lines.

## Round 3, `ec87d1554`

Verdict CONFIRMED. Nothing material left.

### Findings

- **S7-1, note, origin introduced.** The second half of the `strace` case of S6-1 cannot fail
  while a path through the writable mount is refused: the helper never stops with `fs/w/a/d`
  as its directory, the mover never acts, and the case prints `moved false` twice. The
  property holds trivially. A move under the shipped helper's walk was exercised only by the
  new `security7_gate.rs`, which was in no lane.
  - Fixed in `13ce453ed`: read in git at `731d06685`, `security7_gate.rs` is in the tree and
    `scripts/delegated-lane.sh` requires its `executed` line. Not run at that commit.
- **S7-2, note, origin pre-existing.** `MountTable::read_from` had no caller outside
  `place.rs`. `MountTable::place` was called outside the crate by tests only.
  - Fixed in `13ce453ed`: read in git at `731d06685`, `read_from` is private and
    `MountTable::place` is `#[doc(hidden)]`. Not run at that commit.

### Checked and found held

- The window of S6-1 is closed, not moved. A followed path whose walk visits a directory in
  a place a writable location reaches is refused on both sides; the helper holds the
  directories of its walk and drops one for `..`.
  - Soak, 4000 starts per shape, concurrent renamer: the old shape 0 moved; a link through a
    directory that alternates with a link 0 admitted; a link with `..` that never enters the
    writable mount 4000 of 4000 ran the named program alone; a directory swapped for a link
    at the end 0 unsafe.
  - `security7_the_helper_asks_the_kernel_for_no_dot_dot_and_a_moved_directory_does_not_move_the_end`:
    under `strace`, 37 opens before the program and none of `..`; with the directory moved
    while the helper was held, the walk ended where the link's text names.
- Every entry is placed where it ends, in both classes, in the driver and the helper:
  executed by the committed cases.
- A source that overlaps a writable location is asked strictly and refused before dispatch
  where that fails; `class_together` ends after at most one pass per mount plus one and asks
  every mount still classed `follow` on every pass: by reading, and the committed unit cases.
- A base directory that holds the workspace root (a tmpfs at `/usr/local/src`, alone and
  with a stack at `/usr/local/include`): the adapter opened with `exec_subpaths` not proved,
  a listed start was refused `confinement.fact-unserved` at admission, an unlisted start ran.
  Executed.
- The changed assertions in the reviewer's round 2 files keep what the cases were for, with
  the limit S7-1 states.
- The lane's `=<name>` prefix: an absent always-required name exits 1 whether or not mounts
  are granted, on canned logs.

Suite: 778 cases, 0 failed, 0 `ABSENT` lines.

## Test files written by the reviewer that joined the suite

| file | cases | joined in |
|---|---|---|
| `crates/substrate-host/tests/security5_view_mounts.rs` | 2 | `d36436a0f`, untouched |
| `crates/substrate-exec-gate/tests/security6_gate.rs` | 4 | `ec87d1554`, three assertions changed by the implementor, each with a doc paragraph |
| `crates/substrate-host/tests/security6_view_mounts.rs` | 3 | `ec87d1554`, one assertion changed by the implementor, with a doc paragraph |
| `crates/substrate-exec-gate/tests/security7_gate.rs` | 1 | `13ce453ed` |
| `crates/substrate-host/tests/security7_view_mounts.rs` | 1 | `13ce453ed` |

The two `security7` files have in `13ce453ed` the line counts the reviewer wrote, 227 and
272. Inferred from that: unchanged. Not compared byte for byte.

## Not run, in any round

- `bash scripts/gate.sh` and `bash scripts/delegated-lane.sh` end to end; a hosted runner.
  The lane's staged section was run only as its own function on logs.
- Anything at the merge `731d06685` or at `13ce453ed`. Both were read in git only.
- The adversary pass's files of the same round, in each round. Those of earlier rounds ran
  as part of the suite once committed.
- One file system under two device numbers (FUSE, 9p, virtiofs, a network file system
  mounted twice): no stage without privilege. A `ramfs` beneath the root only stood in for
  what the driver sees of it.
- `xfs`, `btrfs`, NFS or FUSE as the type outside the list: `ramfs` stood in.
- The `macos` adapter, and `systemd` around `linux` beyond the committed
  `ess_confinement_systemd` binary in the suite.
- `mount-id-unreported`, a `(deleted)` or escaped mount point through the driver, a host
  whose process root is a chroot.
- The driver's own walk under a concurrent writer.
- A stack the view itself makes: a declared mount placed on a host mount point beneath a
  writable mount.
- The `exec.sandbox-place-unserved` branch for a base directory through a real adapter: the
  open withholds the fact first. Unit cases hold the branch.
- The no-mount branch of the lane on a real host. The simulation
  (`bwrap --dev-bind / / --unshare-user --disable-userns`) is stricter than a hosted runner:
  bubblewrap itself cannot run there.

## Where the evidence is

Scripts and logs of the rounds are under `<scratch>/fifth/`, `<scratch>/sixth/` and
`<scratch>/seventh/`: `suite.log` and one `suite-<binary>.log` per binary, the case logs, the
soak script `race.py` with `race.log`, and for round 2 `race-kernel.log`. The scratch
directory is not durable. The worktree was `<worktree>`; nothing outside it and the scratch
directory was written.

```findings
[
{"file":"crates/substrate-exec-gate/src/place.rs","line":620,"category":"boundary","severity":"warning","verdict":"NEEDS-CHANGE","origin":"pre-existing","message":"S5-1, round 1, line as at f402165ad. A mount of a file-system type outside the list is skipped at all four sites that ask whether something lies where a run can write. When the same such file system is mounted beneath a workspace and beneath a read-only root's host directory, the two do not overlap, the root is classed follow, and a symbolic link a run planted on the mount is followed: the walk places a path only where it ends. Red: security5_a_link_on_a_mount_the_rule_skips_is_not_followed_out_of_a_read_only_root (crates/substrate-host/tests/security5_view_mounts.rs:383), on a ramfs a program of /usr/bin ran where the tmpfs twin was refused. Fixed in d36436a0f: every link the walk reads is placed, and two rows of an unlisted type with one device overlap. Measured there: the case is green."},
{"file":"crates/substrate-exec-gate/src/place.rs","line":728,"category":"surface","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"S5-2, round 1, line as at f402165ad. The helper crate gains more public items than Unestablished.asked and Cause::MountCovered: Asked, Unestablished::place, Unlisted, lies_in_any, overlap, MountTree (table, reaches), MountTable::into_tree, into_tree_reported_by, into_tree_as_written and reported_at. Nothing outside the crate uses reported_at, into_tree_reported_by or Unestablished::place. Reduced in d36436a0f, read in its diff: lies_in_any, overlap, into_tree_reported_by and reported_at are private and Unestablished::place is pub(crate)."},
{"file":"crates/substrate-exec-gate/src/landlock.rs","line":292,"category":"security","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"S6-1, round 2, line as at d36436a0f. The name-by-name walk asks the kernel for `..` from a directory descriptor in a second system call. A second run that shares a --writable mount renames that directory between the two calls, and the walk of a trusted link whose target passes through the mount and leaves it with `..` ends one level off. The execute rule lands on a directory the entry does not name and the driver did not judge. The end is still placed, so it is never in a writable mount. Red: security6_a_concurrent_writer_cannot_move_the_end_of_a_trusted_link_during_the_walk (crates/substrate-exec-gate/tests/security6_gate.rs:486), `126\\n0\\n` where the control is `0\\n126\\n`. Soak without strace: 363 of 4000 starts moved, 0 of 4000 with the kernel's own walk. Fixed in ec87d1554: a followed path that visits a directory in a place a writable location reaches is refused on both sides, and the helper resolves `..` on the directories it holds. Measured there: 0 of 4000 moved, the case is green."},
{"file":"docs/design/35-executable-lists.md","line":4011,"category":"documentation","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"S6-2, round 2, line as at d36436a0f. The amendment says MountTree::reaches_as_written is used for a root's or base directory's source and for the helper's file. The code uses it only for a source (crates/substrate-host/src/exec_gate.rs:859). The helper's file is one place compared with each writable location's fail-closed reach, the safer of the two. Fixed in ec87d1554, read there: the sentence says \"source only\" and states the helper's file separately."},
{"file":"crates/substrate-exec-gate/src/place.rs","line":201,"category":"api-surface","severity":"note","verdict":"CONFIRMED","origin":"pre-existing","message":"S6-3, round 2, line as at d36436a0f. Place::relation, Place::lies_in, Relation, MountTable::lies_in and the free place::place have no user outside place.rs. Reach::places and MountTable::into_tree_as_written are used by tests only. Unlisted::Refused has no production user on the branch. Fixed in ec87d1554, read there: the first three are pub(crate), the next two #[cfg(test)] pub(crate), the two test-only items #[doc(hidden)], and Unlisted::Refused names story 2 as its caller."},
{"file":"scripts/delegated-lane.sh","line":395,"category":"test-lane","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"S6-4, round 2, line as at d36436a0f. The reason the lane takes on any host for `adversary5 exec gate view mounts limit: overlay upper` is `the stage failed`, which every failing stage command prints, not only the overlay mount. It cannot stand in for a failure of the helper, because the stage ends before the helper starts. Fixed in ec87d1554, read there: the stage says when the overlay mount itself was refused and the lane takes only that reason."},
{"file":"crates/substrate-exec-gate/tests/security6_gate.rs","line":505,"category":"test-evidence","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"S7-1, round 3, line as at ec87d1554. The second half of security6_a_concurrent_writer_cannot_move_the_end_of_a_trusted_link_during_the_walk cannot fail while a path through the writable mount is refused: the helper never stops with fs/w/a/d as its directory, the mover never acts, and the case prints `moved false` twice. The property holds, trivially. A move under the shipped helper's walk is exercised by crates/substrate-exec-gate/tests/security7_gate.rs, which was in no lane. Fixed in 13ce453ed, read in git at 731d06685 and not run: that file is in the tree and scripts/delegated-lane.sh requires its executed line."},
{"file":"crates/substrate-exec-gate/src/place.rs","line":384,"category":"api-surface","severity":"note","verdict":"CONFIRMED","origin":"pre-existing","message":"S7-2, round 3, line as at ec87d1554. MountTable::read_from has no caller outside place.rs and can be private. MountTable::place (396) is called outside the crate by tests only. Fixed in 13ce453ed, read in git at 731d06685 and not run: read_from is private and MountTable::place is #[doc(hidden)]."}
]
```
