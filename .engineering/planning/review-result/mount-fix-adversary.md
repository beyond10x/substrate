---
format: aep.planning-md/3
id: review-result:mount-fix-adversary
kind: review-result
status: active
title: 'Adversary pass, three rounds: the mount fix of the allow-list (design 35 D30)'
tags:
- adversary
relations:
- reviews: story:linux-executable-allow-list
revision: 1
---
# Adversary pass on the mount fix (design 35 D30), three rounds

- Reviewer: the `adversary` role of `aep:implementing`. It wrote failing test cases and ran mutants on scratch copies; it edited no implementation file and made no commit.
- Date: 2026-11-21.
- Branch: `fix/lies-in-follows-the-views-mounts`, merged as pull request 244 (`731d06685`).
- Finding ids: `R<round>-<id>`, where `<id>` is the id the round's own report used. The rounds' test files are named `adversary5_*`, `adversary6_*` and `adversary7_*`, after the reviewer's fifth to seventh pass on design 35.
- File and line of a finding are those of the commit the round attacked.

| Round | Commit attacked | Verdict |
|---|---|---|
| 1 | `f402165ad` | 1 blocker, 1 warning, 4 notes. The fix closed the four shapes it named; a planted link on a file system of an unlisted type was still followed. |
| 2 | `d36436a0f` | 2 warnings, 2 notes. The sentence the read-only side rests on held for the class and failed for two outcomes of that class. |
| 3 | `ec87d1554` | 2 notes. Nothing material left. |

## Round 1, on `f402165ad`

### Findings

- **R1-F1, blocker, fixed in `d36436a0f`.** A mount of a type outside the list was skipped on both sides of the class decision. A `ramfs` mounted beneath the workspace root and again beneath a read-only root's source carried a planted link to `/usr/bin`. Through the real `linux` adapter: control `ran "126\n"`, planted `ran "0\n"`. In the helper alone: status 0 where 125 was expected. Origin pre-existing: the same hole on `main` is inferred from the diff, not run.
- **R1-F2, warning, fixed in `d36436a0f`.** A start was refused `exec.sandbox-place-unserved` for a mount beneath a read-only root's source that nothing writable touched. On the review workstation, with no staged mount, a root at `/var/lib` was refused `path-unopenable` for a container layer mount the user could not open. Staged, two tmpfs stacked beneath a root were refused `mount-covered`. That `main` ran this start is inferred; bubblewrap alone bound `/var/lib` read-only with exit 0.
- **R1-F3, note, fixed in `d36436a0f`.** `MountTree::beneath` recursed once per mount of a chain. 10 000 mounts stacked beneath a directory, written deepest first, aborted the process on a 2 MiB stack in debug and in release; 2 000 passed.
- **R1-F4, note, fixed in `d36436a0f`.** `reaches` accepted a mount ID that two non-parent rows share and returned both roots. A kernel-written table cannot contain this.
- **R1-F5, note, open (a measurement, no change asked).** Linking the table is quadratic in rows: 10 000 rows side by side linked in 139 ms in release and 1.24 s in debug; one directory was asked in 2.7 ms and 15 ms. Not measured again on the later commits.
- **R1-F6, note, not measured again.** The round's report listed this without an id, in its findings block only. Of 38 mutants (28 on the helper crate, 10 on the host crate, all applied) one survived every suite: `O_NOFOLLOW` removed from `reported_at`, likely equivalent. `Unlisted::Skipped` in the helper was held by no test of the helper crate, only by the lane: with `Refused` all ten `adversary_exec_lists` cases failed.

### Measured, held

- Beneath a writable mount the helper reached: a single bound file; four tmpfs in a chain; a mount point named with a space, a tab, a newline, a backslash and ` (deleted)`; a tmpfs stacked on a `devpts` and a file bound onto a `devpts` node; a writable root that is itself a mount point; a writable root that is a recursive bind of a directory holding a tmpfs.
- A stack and a hidden mount beneath the writable mount ended the helper 125 `mount-covered`; the same beneath a read-only directory refused nothing.
- Through the adapter: a root on the source directory of a bind beneath the workspace root was refused `exec.sandbox-path-unrepresentable`; a workspace root that is a mount point held; `/usr` with a tmpfs on `/usr/local` ran as before.
- Refused before any of this, pre-existing: a workspace that is its own mount point (`workspace.path-escape`), a root at `/` (`exec.read-only-root-invalid`).
- Hostile tables: circles of two and three rows, a row that is its own parent away from `/`, two parentless rows at `/`, an absent parent, and `/a/bc` against `/a/b` all refused or compared by name.
- The lane script's branch for a host that grants a user namespace no mount, run from the script's own lines inside `bwrap --unshare-user --disable-userns`: each binary needed its own `ABSENT (no …` line. No green path without one was found.
- The stated limit, measured: an overlay whose upper directory the run writes was followed and ran the rewritten bytes.

## Round 2, on `d36436a0f`

The stakeholder's target was the sentence that counting a mount as reached "can only make a class stricter".

### Findings

- **R2-F1, warning, fixed in `ec87d1554`.** A root the count classed `beneath` was passed to the helper as `--writable`, and the helper's writable side fails closed. Every start with that root, the control included, ended 125 after dispatch: `cannot establish where /opt/k/hid is attached: mount-covered` for a covered bind of a workspace directory, and `… /opt/k/locked/m is attached: path-unopenable` for the same bind under a directory closed to the user.
- **R2-F2, warning, fixed in `ec87d1554`.** The counted class changed an entry's answer. An entry on a `ramfs` that only the root shows was refused `exec.sandbox-place-unserved` alone, and ran (`ran "0\n"`) once a covered second mount of a `ramfs` beneath the workspace root lay beside it. That this is harmful only where the same files are writable under a device the row comparison cannot see is inferred; that shape was not staged.
- **R2-F3, note, fixed in `ec87d1554`** by the round's case joining the suite. No shipped test held the helper's bound on links: `links > MAX_LINKS + 1` and `links >= MAX_LINKS` both survived.
- **R2-F4, note, reported as taken in `ec87d1554`, not measured again.** The call of `judge_link` in the driver's walk was held by no unit test, only by staged cases.

### Measured, held

- 3 068 generated mount tables (stacks, hidden rows, removed roots, repeated IDs, two unlisted types) gave 334 808 pairs of a source and a writable location. In 285 325 the kernel's answer could be had and agreed with the table's. In 44 793 the kernel refused and the table's answer overlapped. In none did the kernel's answer overlap where the table's did not.
- A hidden mount with the hiding mount shown, and a removed root, were counted and classed the source `beneath`.
- Unlisted rows are compared by the device they name: same device and nested roots overlap; sibling btrfs subvolumes do not; two superblocks of one export, with two devices, do not.
- A link on a `ramfs` was refused `exec.sandbox-place-unserved` by the driver, and ended the helper 125, as last name, as middle name, through another link, through `..` in a target and with an absolute target. 40 links ran and 41 were refused. `/proc/<pid>/cwd`, `/proc/<pid>/root` and `/proc/self/cwd` ended the helper 125.
- Without `--writable` the helper followed the `ramfs` link, as its documentation says. That no start is made that way is read from the driver's code.
- 27 mutants (21 on the helper crate, 6 on the host crate, all applied): all caught except the two of R2-F3; the one of R2-F4 only by a staged case.

## Round 3, on `ec87d1554`

### Findings

- **R3-N1, note, open, filed as `story:linux-adapter-names-why-the-list-fact-is-absent`.** With a directory of the workspace root bound at `/usr/local`, the probe's plan is refused, the adapter opens without `exec_subpaths` and its proof names no cause (`withheld: []`). Both starts were refused `confinement.fact-unserved` at admission; no helper run ended 125. Origin pre-existing is inferred: the earlier commits were not run.
- **R3-N2, note, fixed in `13ce453ed`** by the round's case joining the suite; the commit's diff was not read, the file is in `731d06685`. With the walk's skip of a `.` name removed, the whole shipped suite of `ec87d1554` passed, and `dir/./../dir` resolved to `dir/dir` in the helper where the driver resolves `dir`.

### Measured, held

- The two assertions the implementor changed in the round-2 files hold what the decisions say.
- A chain of roots (`a` overlaps the workspace root, `b` only `a`, `c` only `b`) was refused `exec.sandbox-path-unrepresentable` in both orders of declaration; `c` alone ran.
- On the review workstation a root at `/var/lib` ran.
- In the workspace, an entry on a `ramfs` and the `ramfs` mount point were `exec.sandbox-place-unserved`; an entry on a tmpfs and the tmpfs mount point ran; the program and the loader as entries ran; an entry on the view's `/tmp` was `exec.sandbox-exec-subpath-undeclared`.
- The driver's walk and the helper's ended at the same file for `x/a/../bin` with `x/a` a link, `/../../usr/bin`, `/usr/./bin//./`, `../../usr/bin` and six `..` above the root. `/workspace/sub/../../usr/bin`, and a link to that link, were refused before dispatch. No start was admitted by the driver and ended by the helper.
- 18 mutants (9 on the helper crate, 9 on the host crate, all applied): all caught by shipped tests except the one of R3-N2.

## Test files that joined the suite

All eight are in `731d06685`.

| File | Joined in |
|---|---|
| `crates/substrate-exec-gate/tests/adversary5_mount_tree.rs` | `d36436a0f` |
| `crates/substrate-exec-gate/tests/adversary5_view_mounts_gate.rs` | `d36436a0f` |
| `crates/substrate-host/tests/adversary5_view_mounts.rs` | `d36436a0f` |
| `crates/substrate-exec-gate/tests/adversary6_reach.rs` | `ec87d1554` |
| `crates/substrate-exec-gate/tests/adversary6_links_gate.rs` | `ec87d1554` |
| `crates/substrate-host/tests/adversary6_view_mounts.rs` | `ec87d1554` |
| `crates/substrate-exec-gate/tests/adversary7_walk_gate.rs` | `13ce453ed` (read from the log) |
| `crates/substrate-host/tests/adversary7_view_mounts.rs` | `13ce453ed` (read from the log) |

The implementor changed these files after the reviewer wrote them, and said so each time: three tables of `adversary5_mount_tree.rs` are linked with `into_tree_as_written()`; `adversary6_a_root_the_count_classes_beneath_…` expects a refusal before dispatch; the row `through a writable directory by ..` of `adversary6_links_gate.rs` expects the refusal. The reviewer read each change and confirmed the cases still prove what they proved.

## Not run

- `main` for R1-F1 and R1-F2, and the earlier commits for R3-N1: no second checkout.
- The whole workspace suite and `scripts/gate.sh`, in any round.
- A hosted runner. The lane's branch for a host that grants no mount was simulated only.
- `systemd` around `linux`, and `macos`.
- Two superblocks of the same writable files (NFS, FUSE): no stage without privilege.
- Shared mount propagation: every staged case ran on the private propagation `unshare --mount` gives.
- Mounts made after the table was read, and a race between the driver's walk and the helper's: no case.
- A table of more than 10 000 rows.
- The soak of the helper's walk is not this reviewer's measurement. The 363 of 4 000 starts that ended one directory off before the third commit are in the design text and were read there; the 0 of 4 000 after it is from the coordinator's message and was not seen by this reviewer.

## Where the evidence is

The rounds' logs and mutation scripts were kept under `<scratch>/adversary1/` (`logs/a5-*.log`, `a6-*.log`, `a7-*.log`, `mutate5-*.log`, `mutate6-*.log`, `mutate7-*.log`); that directory is deleted 48 hours after the session ends. The cases themselves are in the repository.

```findings
[
{"file":"crates/substrate-exec-gate/src/place.rs","line":622,"category":"security","severity":"blocker","verdict":"NEEDS-CHANGE","origin":"pre-existing","message":"R1-F1: at f402165ad a mount of a type outside the list is skipped on both sides of the class decision. A ramfs mounted beneath the workspace root and again beneath a read-only root's source carries a link a run planted; the root is classed follow, the driver and the helper follow the link, and a program of /usr/bin runs (control ran 126, planted ran 0, through the real adapter). The link is never placed, only its target. The same hole on main is inferred from the diff. Fixed in d36436a0f."},
{"file":"crates/substrate-host/src/exec_gate.rs","line":855,"category":"compatibility","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"R1-F2: at f402165ad classed() refuses a start exec.sandbox-place-unserved for a covered or unopenable mount beneath a read-only root's or base directory's source with nothing writable near it. On the review workstation a root at /var/lib was refused path-unopenable for a container layer mount; a staged stack beneath a root was refused mount-covered. That main ran this start is inferred. Fixed in d36436a0f."},
{"file":"crates/substrate-exec-gate/src/place.rs","line":579,"category":"robustness","severity":"note","verdict":"NEEDS-CHANGE","origin":"introduced","message":"R1-F3: at f402165ad MountTree::beneath recurses once per mount of a chain; 10 000 stacked mounts written deepest first abort the process on a 2 MiB stack in debug and release, 2 000 pass. Needs a table only host root can build. Fixed in d36436a0f."},
{"file":"crates/substrate-exec-gate/src/place.rs","line":633,"category":"correctness","severity":"note","verdict":"NEEDS-CHANGE","origin":"introduced","message":"R1-F4: at f402165ad reaches accepts a mount ID that two non-parent rows have and returns both roots instead of mount-row-repeated. Not reachable with a kernel-written table. Fixed in d36436a0f."},
{"file":"crates/substrate-exec-gate/src/place.rs","line":468,"category":"performance","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"R1-F5: at f402165ad linking the table is quadratic in rows: 10 000 rows link in 139 ms in release and 1.24 s in debug, once per open, per start and per helper; one directory is asked in 2.7 ms and 15 ms. A measurement, no change asked; not measured again on the later commits. Open."},
{"file":"crates/substrate-exec-gate/src/place.rs","line":735,"category":"test-gap","severity":"note","verdict":"NEEDS-CHANGE","origin":"introduced","message":"R1-F6: at f402165ad, of 38 mutants one survives every suite: O_NOFOLLOW removed from reported_at, likely equivalent. Unlisted::Skipped in the helper (landlock.rs:233) is held by no test of the helper crate, only by the lane through the probe: with Refused all ten adversary_exec_lists cases fail. The round's report gave this entry no id. Not measured again."},
{"file":"crates/substrate-host/src/exec_gate.rs","line":859,"category":"compatibility","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"R2-F1: at d36436a0f a root that reaches_as_written classes beneath for a covered or unopenable mount no run can reach is passed to the helper as --writable (line 1092), and the helper's writable side fails closed: every start with that root, the control included, ends 125 after dispatch (mount-covered for a covered bind of a workspace directory, path-unopenable for the same bind under a directory closed to the user). Fixed in ec87d1554."},
{"file":"crates/substrate-host/src/exec_gate.rs","line":859,"category":"correctness","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"R2-F2: at d36436a0f the counted class changes an entry's answer: an entry on a ramfs only the root shows is refused exec.sandbox-place-unserved alone, and runs once a covered second mount of a ramfs beneath the workspace root lies beside it, because a beneath entry is opened and never placed (lines 1441-1449). That it is harmful only where the same files are writable under a device the row comparison cannot see is inferred. Fixed in ec87d1554."},
{"file":"crates/substrate-exec-gate/src/landlock.rs","line":298,"category":"test-gap","severity":"note","verdict":"NEEDS-CHANGE","origin":"introduced","message":"R2-F3: at d36436a0f no shipped test holds the helper's bound on links: links > MAX_LINKS + 1 and links >= MAX_LINKS survive the shipped suite. The other 19 helper mutants and all 6 host mutants are caught. Fixed in ec87d1554 by the round's case adversary6_the_walk_follows_what_the_kernel_follows joining the suite."},
{"file":"crates/substrate-host/src/exec_gate.rs","line":1308,"category":"test-gap","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"R2-F4: at d36436a0f the call of judge_link in follow() is held by no unit test, only by staged cases; on a host that grants a user namespace no mount nothing executed holds it. Reported as taken in ec87d1554; not measured again."},
{"file":"crates/substrate-host/src/probe.rs","line":529,"category":"observability","severity":"note","verdict":"CONFIRMED","origin":"pre-existing","message":"R3-N1: at ec87d1554, with a directory of the workspace root bound at /usr/local, the probe's plan is refused, the adapter opens without exec_subpaths and its proof names no cause (withheld is empty); every start with a list is refused confinement.fact-unserved at admission, and no helper run ends 125. Origin is inferred: the earlier commits were not run. Open, filed as story:linux-adapter-names-why-the-list-fact-is-absent."},
{"file":"crates/substrate-exec-gate/src/landlock.rs","line":333,"category":"test-gap","severity":"note","verdict":"NEEDS-CHANGE","origin":"introduced","message":"R3-N2: at ec87d1554 the walk's skip of a '.' name is held by no shipped test: removed, dir/./../dir resolves to dir/dir in the helper and to dir in the driver. The other 8 helper mutants and all 9 host mutants of the round are caught by shipped tests. Fixed in 13ce453ed by the round's case adversary7_dot_dot_is_taken_from_where_the_walk_stands joining the suite; the file is in 731d06685."}
]
```
