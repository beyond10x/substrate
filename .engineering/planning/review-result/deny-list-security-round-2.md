---
format: aep.planning-md/3
id: review-result:deny-list-security-round-2
kind: review-result
status: active
title: 'Security review, round 2: the executable deny-list'
relations:
- reviews: story:executable-deny-list
revision: 1
---
verdict: NEEDS-CHANGE at `4653b2f9c` (story 2, round 2). No denied file executed in anything I staged. Two warnings: the public text understates the created-later limit, and one start the driver could refuse is dispatched and ends 125. This is a review, not an approval, and it is not independent of my earlier looks.

## Per invariant

| # | answer | site | case |
|---|---|---|---|
| 1 | **holds** for execution; driver and helper **disagree in one shape** (S8-2) | `crates/substrate-exec-gate/src/landlock.rs:526` (`step`), `crates/substrate-host/src/exec_gate.rs:1878` (`judge_denied`) | `security5_a_denied_file_is_denied_under_the_names_mounts_give_it`: denied mount point `vol 126`, its second mount `again 126`, file bind `name 126`, symlink entry `link 126`, neighbours 0. Committed `exec_denied_view_mounts`, `security4_*`, `adversary4_*` green |
| 2 | **fails closed** everywhere; **not refused before dispatch** where the driver could know (S8-2); one run-caused 125 (S8-3) | `landlock.rs:805`, `landlock.rs:694`, `exec_gate.rs:1906` | same case, row `unlisted entry`; `security5_a_directory_an_earlier_run_made_unreadable_runs_nothing_of_it` |
| 3 | **holds** | `crates/substrate-host/src/probe.rs:621`, `:625`, `:655` | the static helper built from `aafb9823a` (places alone): the open proved `ExecSubpaths` and not `ExecDeniedPaths`. The probe's view built by hand with bubblewrap: that helper ends the script with status 25 (`apart/held/tool` ran), the current helper with 0 |
| 4 | **holds** | `crates/substrate-exec-gate/src/lib.rs:313`, `crates/substrate-host/src/confinement/mod.rs:644`, `exec_gate.rs:1682`, `:1729` | committed `security4 exec denied trace` (bubblewrap and the helper only before the ruleset), lib parse cases (60), `deny-only` bound case, `exec gate deny entry into a writable mount`, my `link entry` row |
| 5 | **does not hold** (S8-1) | `website/docs/guides/confinement-profiles.md:278`, `:281-288`; `crates/substrate-exec-gate/README.md:130-136`; `CHANGELOG.md:39-42` | `security5_a_new_file_under_an_absent_deny_entrys_name_is_denied_whatever_the_run_renames`, red |
| 6 | **holds**, with generic reasons (S8-4) | `scripts/delegated-lane.sh:336`, `:477`, `:493-494` | all 38 committed deny-list case names are in the lane; its `require_staged` cut from the script and run on canned logs (`=` absent exits 1, `?` only for its own reason) |

On item 2, the implementor's named case: I decide it is against this commit's own rule. `judge_denied` exists so that the driver asks what the helper asks; here it does not. A run cannot cause it for another run: it needs a host mount, and a run cannot mount.

A program in a declared mount beneath a denied directory cannot be staged: each of three shapes is refused at admission (`exec.read-only-root-invalid` twice, `exec.read-only-file-mount-overlap`). So in those shapes the driver's own table misses no view mount.

## Findings

**S8-1, warning.** A new file under an absent deny entry's name runs once the run renames a ruled directory onto the entry's parent.
- Command: `bash <scratch>/eighth/cases.sh security5_exec_denied security5_a_new_file_under_an_absent_deny_entrys_name_is_denied_whatever_the_run_renames`
- Lines:
  ```
  ran Some(0) "listed 0 new directory 126 copied 126 copied after a rename 0 a link to a program of another listed directory 0"
  an entry that exists: ran Some(0) "denied 126 new file in a directory of the expanded one 0 the denied path after a rename 0 the denied file 126"
  panicked at crates/substrate-host/tests/security5_exec_denied.rs:270:5 … left: Some("copied after a rename 0") right: Some("copied after a rename 126")
  ```
- Why: a subdirectory of an expanded directory that holds no denied path gets one rule on the directory. Everything created in it executes, and the rule moves with a rename.
- What the text says against it:
  - "a new file under that name is denied";
  - "A file created there during the run is not executable";
  - "No program outside the list runs this way" (the copied file came from an unlisted directory);
  - "of the same directory" (a symbolic link to a program of another listed directory ran).
- Bound: nothing ran that the run could not already run by copying into that subdirectory. The denied file itself stayed 126.
- Committed figures reproduced: `copied 126`, `linked 0`, `renamed 0`; from an unlisted directory `linked 126`, `renamed 126`, `copied 126`.

**S8-2, warning.** A mount of an unlisted type that is an entry of an expanded directory: dispatched, ended 125.
- Command: same script, case `security5_a_denied_file_is_denied_under_the_names_mounts_give_it`.
- Lines:
  ```
  unlisted entry: ran Some(125) "" "substrate-exec-gate: cannot establish the place of /opt/k/odd/media: file-system-unlisted ramfs"
  unlisted beside: ran Some(0) "prog 0 media 0 tool 126"
  ```
- The driver has the row, the listed place and the denied place in its own table. `exec_gate.rs:1906` asks with `Unlisted::Skipped`; the helper places the mount point as a child at `landlock.rs:805`.

**S8-3, note.** An earlier run takes read permission from a directory on the way to a deny entry; every later start of that profile ends 125.
- Lines: `before: ran Some(0) "denied 126 beside 0"`, `after: ran Some(125) "" "… cannot read the directory \"/workspace/bin/held\": Permission denied"`.
- Nothing runs. Same workspace, so the same tenant.

**S8-4, note.** Three lane reasons are generic.
- `?security4 exec denied unplaced|this host` also matches `the root alone is refused on this host: <any refusal>`, which would hide a regression of a root at `/boot`.
- `the stage failed` is taken for both overlay cases of `namespace_gate`.

**S8-5, note.** `the_place_rule_holds_on_an_overlay` took 6.86 s run alone (`namespace_gate` whole: 18.6 s under load). It is over the 5 s line and not named with its time.

## Test file written

`crates/substrate-host/tests/security5_exec_denied.rs` (741 lines, untracked, `rustfmt` and `clippy -D warnings` clean, no machine-local path). Whole binary: 1.46 s.

| case | result | time alone |
|---|---|---|
| `security5_a_new_file_under_an_absent_deny_entrys_name_is_denied_whatever_the_run_renames` | **FAILED** | 0.15 s when first run, one start; it now makes two and was not timed alone again |
| `security5_a_directory_an_earlier_run_made_unreadable_runs_nothing_of_it` | ok | 0.23 s |
| `security5_a_program_in_a_declared_mount_beneath_a_denied_directory_is_refused` | ok | 0.13 s |
| `security5_a_denied_file_is_denied_under_the_names_mounts_give_it` | ok | 0.39 s |

The adapter is opened once per case, not once per binary; each open is under 0.3 s.

Suite, by name, release profile, `CARGO_INCREMENTAL=0`: 300 committed cases green, 304 with mine, 1 red.
- **Helper crate:** lib 60, `gate` 24, `namespace_gate` 4, `adversary4_gate` 4, `adversary4_namespace_gate` 5, `security4_gate` 3.
- **Host crate:** `ess_confinement_linux` 91 (28 s), `exec_denied_view_mounts` 1, `security4_exec_denied` 3, `adversary4_exec_denied` 10, lib 95 of 544 by filter (`exec_gate`, `probe`, `confinement::linux`, `exec_list`).
- **ABSENT:** one line, the adversary's nested host mount, as the lane expects.

## Not run

- `cargo test --workspace`, the delegated lane end to end, `scripts/gate.sh`, a hosted runner.
- The adversary's untracked `adversary5_*` files.
- `macos`, and `systemd` around `linux`.
- S8-1 at `aafb9823a`. The mechanism is older by reading; the public text is new in this commit.
- A `Scoped` workspace with a deny entry in a writable subtree bind; a dangling symbolic link as a deny entry.
- The probe's exit status inside the adapter. The 25 is from the hand-built view.
- The no-mount branch of the lane on a real host. In my `bwrap --disable-userns` simulation bubblewrap itself cannot run, and `security4_exec_denied` and `adversary4_exec_denied` exit 101 there; that says nothing about a hosted runner.

Needs coordinator: `security5_exec_denied.rs` is in no lane. Its red case passes once a new file under the entry's name is denied after the rename; if the limit is stated instead, the assertion at `:270` changes with it.

Written outside the worktree, all under `<scratch>/eighth/`: `warm.sh`, `cases.sh`, `suite.sh`, `probe-by-hand.sh`, their logs, `impl.diff`, `suite.paths`, the two helper copies. No target directory or second profile was made. Lease taken and released.

```findings
- file: website/docs/guides/confinement-profiles.md
  line: 278
  category: documentation
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: >-
    S8-1. The stated created-later limit is not the whole limit. Guide 278 and 281-288, helper
    README 130-136 and CHANGELOG 39-42 say a new file under the name of a deny entry that was
    absent at the start is denied (`copied 126`), that a file created in a listed directory
    that holds a denied path is not executable, and that no program outside the list runs this
    way. Measured: with bin listed and bin/sub/tool denied and absent, the run renames bin/lib,
    a directory of bin that got its own rule, to bin/sub, copies a program of an unlisted
    directory to bin/sub/tool and runs it: `copied after a rename 0`. Red:
    security5_a_new_file_under_an_absent_deny_entrys_name_is_denied_whatever_the_run_renames
    (crates/substrate-host/tests/security5_exec_denied.rs:270). Printed beside it: a symbolic
    link under an absent entry's name to a program of another listed directory ran (0); under
    an entry that exists, a new file at the denied path after the same rename ran (0), a new
    file in a subdirectory of the expanded directory ran (0), and the denied file stayed 126.
    Nothing ran that the run could not already run by copying into that subdirectory. Either
    the text states that a rule on a directory inside an expanded directory admits what is
    created in it and moves with a rename, or the expansion gives no directory rule there.
- file: crates/substrate-host/src/exec_gate.rs
  line: 1906
  category: boundary
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: >-
    S8-2. The driver and the helper disagree in one shape. A mount of a file-system type
    outside the list that is an entry of a directory the helper expands is asked by
    judge_denied with Unlisted::Skipped and refuses nothing; the helper places it as a child
    (crates/substrate-exec-gate/src/landlock.rs:805) and stops. Measured in
    security5_a_denied_file_is_denied_under_the_names_mounts_give_it: with /opt/k/odd listed,
    /opt/k/odd/prog denied and a ramfs at odd/media the start is dispatched and ends 125 with
    `cannot establish the place of /opt/k/odd/media: file-system-unlisted ramfs`; nothing
    runs. The driver's own table has the row, the listed place and the denied place, so the
    start can be refused exec.sandbox-place-unserved before dispatch, as this commit does for
    a covered mount. A run cannot cause it for another run: it needs a host mount.
- file: crates/substrate-exec-gate/src/landlock.rs
  line: 694
  category: availability
  severity: note
  verdict: CONFIRMED
  origin: pre-existing
  message: >-
    S8-3. A directory the expansion cannot read ends the start with status 125 after dispatch,
    and a run can cause it for later starts of the same workspace: an earlier run without a
    deny-list takes the read permission from bin/held, and the start with bin listed and
    bin/held/tool denied then ends `cannot read the directory "/workspace/bin/held":
    Permission denied`. Nothing runs. Measured in
    security5_a_directory_an_earlier_run_made_unreadable_runs_nothing_of_it; the control
    before the chmod prints `denied 126 beside 0`.
- file: scripts/delegated-lane.sh
  line: 477
  category: test-lane
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: >-
    S8-4. Three reasons the lane takes on any host are generic. `?security4 exec denied
    unplaced|this host` matches `this host has no vfat mount at /boot/efi` and also `the root
    alone is refused on this host: <any refusal>`
    (crates/substrate-host/tests/security4_exec_denied.rs:344), which would hide a regression
    of a read-only root at /boot. Lines 493 and 494 take `the stage failed` for the two
    overlay cases of namespace_gate, which every failing stage command prints. A reason that
    names the missing thing would be exact.
- file: crates/substrate-exec-gate/tests/namespace_gate.rs
  line: 369
  category: test-speed
  severity: note
  verdict: CONFIRMED
  origin: pre-existing
  message: >-
    S8-5. the_place_rule_holds_on_an_overlay took 6.86 s run alone, and the namespace_gate
    binary 18.6 s under load, on the review workstation. It is over the 5 s line of "Tests
    stay fast" and is not named with its time. The other three cases of the binary took 0.31
    to 0.34 s each.
```
