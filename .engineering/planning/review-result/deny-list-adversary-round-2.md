---
format: aep.planning-md/3
id: review-result:deny-list-adversary-round-2
kind: review-result
status: active
title: 'Adversary pass, round 2: the executable deny-list'
relations:
- reviews: story:executable-deny-list
prose_only: the findings are the numbered items of the body; the pass returned no machine-readable list
revision: 1
---
## Verdict

`NEEDS-CHANGE` at `4653b2f9c`, for one finding (A8-F1). The coordinator reports that the implementor's uncommitted round 3 closes it; I did not run anything against round 3. With that fix confirmed, nothing material is left from this round.

## What was measured on which state

- **Committed `4653b2f9c`:** the helper-level binary, the item 1 probe run, the soak, and mutants 1 to 8. `landlock.rs` was unchanged throughout.
- **Tree as it was at 23:53 on 2026-11-21:** the host lane binary. The round-3 refusal was not in it; the observations show the helper's status 125.
- **Mixed state:** mutants 9 to 11 were built from committed `landlock.rs` with the uncommitted `lib.rs` of 00:39 (the unit count went from 60 to 61).
- **Round 3 (uncommitted):** nothing run by me.

## Findings

**A8-F1, warning, origin introduced. An unlisted-type mount that is a direct entry of an expanded directory is dispatched and ended by the helper.**
- Command: `lane-a8-bin.sh a8-host-direct <target/debug/deps/adversary5_exec_denied_round2-…>`
- `beside-unlisted: ran Some(125) "" | substrate-exec-gate: cannot establish the place of /opt/t/odd/media: file-system-unlisted ramfs`
- `beside-unlisted-absent`: the same line, although its deny entry names nothing that exists.
- `listed-above-unlisted`: the same, naming `/opt/t/nest/media`.
- `deeper-unlisted: ran Some(0) "denied:126\nbeside:0\n"`: the same mount one directory deeper runs.
- Helper alone: `status Some(125) … /k/vol: file-system-unlisted ramfs`.
- A plausible operator start: a declared root that holds an NFS, vfat or 9p mount point beside the denied file.
- Nothing denied ran. The defect is the layer: D30 rule 3 and § 12 promise a named refusal before dispatch, and § 8.2 excepts only the rule bound.
- **My position on your question: the expectation turns.** I do not hold that the helper alone should run such a start. A rule on a mount point it cannot place is unsafe, because a listed-type mount can sit beneath it and hold a denied file. My case `…listed_mount_above_an_unlisted_one_never_runs` stages exactly that, and the helper ends it 125.
- I turned the helper-level case myself: it is now `adversary5_an_unlisted_mount_among_the_entries_of_an_expanded_directory_runs_nothing` and asserts status 125, empty output and the cause. It passes `rustfmt --check`; it was not compiled or run after the edit.

**A8-F2, note, judgement. D30's text and the decision disagree, and the two shapes answer differently.**
- § 6 says an unlisted mount is "not compared and, but for the last row, not refused", and rule 6 rejects the blanket refusal.
- After round 3, `k/vol` beside the deny entry refuses while `far/in/vol` runs. The design and the guide should say that the difference is where the deny entry is.
- I did not read the uncommitted round-3 document edit.

**A8-F3, note. Item 1: the probe's placement is right, and it proves only one half.**
- Command: `lane-a8-probe.sh`, shipped probe, static mutant helpers.

| Helper | `exec_denied_paths` |
|---|---|
| shipped | proved |
| step 2 compares places alone | withheld (`facts: [… ExecSubpaths, StdinPipe, OutputLive]`) |
| both steps compare places alone | withheld |
| step 1 alone compares places | proved |
| a mount point takes its directory's place | proved |

- The last two are held by shipped cases, and § 9 states the probe does not prove a listed mount beneath a denied directory.
- Not measured: whether the design's own shape (bind beneath `allowed`) would have let the place-only helper through.

**A8-F4, note. Item 5: the ENOENT branch is reachable by a concurrent run; no case can hold it deterministically.**
- Command: `soak8-run.sh`; 3002 entries, a second process creating and removing one name.
- Shipped: `300 x status 0 126 0`. Without the branch: `240 x status 0`, `60 x status 125 … cannot open ".../k/churn": No such file or directory`.
- This is local evidence, not a test.
- A rule on a symbolic link's own inode: no observable difference by reading; no case caught the mutant. The kernel's walk never passes a link's inode; only the rule count changes.
- `fstat` failing on an open descriptor: not reachable by a run, by reading.

**A8-F5, note. Items 3 and 4: no route found.**
- Command: `lane-a8-bin.sh a8-alone-adversary5_no_other_way_to_a_denied_file_runs_it … --exact adversary5_no_other_way_to_a_denied_file_runs_it`
- `routes: … "fd 126\npid-fd 126\nlink-into-other 1\nother-by-fd 127\nsame-directory 126\nmount -\nno-namespace 0\nneighbour 0\n"`
- `python: … "execveat-read 100\nexecveat-path 100\nunnamed-by-descriptor 100\nunnamed-linked 100\n"` (100 is `EACCES`; the last two are `O_TMPFILE`).
- A run makes no mount, so it cannot cause the stack refusal or plant a mount for another run.
- A denied file-system root holds under a bind of its part: `denied-root: ran Some(0) "denied:126\nbeside:0\n"`; helper level `126\n126\n126\n126\n0\n0\n`.
- A mount appearing between the driver's read and the helper's was not staged; the helper reads its own table (judgement).
- `limit-earlier-run: "entry 126\nkept 0\n"`: a hard link made by an earlier start without the deny entry runs. That is the stated limit N4, but § 8.2's "the run cannot link the denied file into another listed directory" holds only for the run that has the entry.

**A8-F6, note. Item 6: the three changed assertions follow the design.**
- The host mount case and the non-UTF-8 case are stricter than before (one outcome each, § 16 E20 and § 8.2).
- The created-later case asserts § 8.2 and § 8.5 as written: `copied 126`, `linked 0`, `renamed 0`.
- Its second half accepts `126` or `127` per line, where the commit message says "126 each".

**A8-F7, note, undecided.** One run of the host binary through `cargo test` in my wrapper proved no fact at all (`facts: []`, both cases `ABSENT`). The same binary started directly in a delegated scope a minute later executed. Cause not established.

## Test files

`crates/substrate-exec-gate/tests/adversary5_denied_reach_gate.rs` — 6 cases, 0.06 s together at `4653b2f9c`:

| Case | Result |
|---|---|
| `…a_bind_beneath_a_second_listed_directory_holds_its_deny_entry` | pass |
| `…a_bind_beneath_the_expanded_listed_directory_holds_its_deny_entry` | pass |
| `…an_unlisted_mount_among_the_entries_of_an_expanded_directory_…` | red as written; expectation turned, not rerun |
| `…a_denied_file_in_a_listed_mount_above_an_unlisted_one_never_runs` | pass (status 125) |
| `…a_denied_root_of_a_file_system_is_denied_under_every_bind_of_its_parts` | pass |
| `…a_deny_entry_beneath_a_file_denies_nothing_and_ends_no_start` | pass |

`crates/substrate-host/tests/adversary5_exec_denied_round2.rs` — 2 cases:

| Case | Result | Time |
|---|---|---|
| `adversary5_no_other_way_to_a_denied_file_runs_it` | pass | 0.76 s |
| `adversary5_a_deny_entry_beside_a_mount_the_helper_cannot_place` | red on A8-F1 (three observations) | 0.78 s |

You report 2 passed on round 3; I did not run that. Neither file is in `scripts/delegated-lane.sh`.

## Mutants

11 of 13 helper mutants ran; the 12th was building when I stopped the run. No mutant was caught by my cases alone: every caught one was already held by the shipped suite.

- **Held by the shipped suite (9):** step 2 places alone; step 1 places alone; both; unlisted beneath denied skipped; unlisted beneath listed refuses; a non-text name always gets its rule; a non-text name gets no rule; a mount point takes its directory's place; another name not asked in step 1.
- **Survivors (2), both named by the implementor:** `an-entry-gone-since-the-read-ends-the-start` (see A8-F4) and `a-symbolic-link-gets-a-rule`.

## Not run

- Helper mutants `an-absent-entry-takes-its-ancestors-identity` and `a-file-is-asked-what-it-reaches`.
- All seven host mutants of `judge_denied`: they need a second target directory, which you ruled out. `mutate8-host.sh` is written and was never started.
- Anything on round 3, including the turned case.
- The workspace suite, the whole delegated lane, the gate and clippy.

## Housekeeping

- Mutants were built with `rustc` directly through the pool into one scratch directory, with no cargo target; it is removed.
- That run left 131 mode-000 overlay work directories from the shipped `namespace_gate` overlay case in my scratch; I made them readable and removed them.
- Scratch is 17 MB: scripts and logs in the session scratch directory, not kept.
- Lease `adversary8-ef94b977` is released. No commit, no store write, no implementation edit.

## Findings block

```json
[
  {"file": "crates/substrate-host/src/exec_gate.rs", "line": 1878, "category": "correctness", "severity": "warning", "verdict": "CONFIRMED", "origin": "introduced", "message": "A8-F1: at 4653b2f9c a start whose expanded listed directory holds a mount of an unlisted file-system type as a direct entry is dispatched and ended by the helper with status 125 (file-system-unlisted), instead of a named refusal before dispatch; judge_denied does not look at the entries the helper will expand. Observed: beside-unlisted, beside-unlisted-absent and listed-above-unlisted each 'ran Some(125) \"\"'; the same mount one directory deeper runs. Nothing denied ran. The coordinator reports the uncommitted round 3 refuses it exec.sandbox-place-unserved; not run by the adversary."},
  {"file": "docs/design/35-executable-lists.md", "line": 1560, "category": "documentation", "severity": "note", "verdict": "NEEDS-CHANGE", "origin": "introduced", "message": "A8-F2: judgement. D30 says a mount of a type outside the list beneath a listed directory is not refused, and rule 6 rejects the blanket refusal; the round-3 decision refuses it where it is a direct entry of an expanded directory, while one directory deeper it runs. The design and the guide should state that the difference is where the deny entry is. The uncommitted round-3 text was not read."},
  {"file": "crates/substrate-host/src/probe.rs", "line": 1, "category": "test-gap", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "A8-F3: the shipped deny probe withholds exec_denied_paths from a helper whose step 2 compares places alone, alone or with step 1 (facts end ExecSubpaths, StdinPipe, OutputLive), so the bind beneath apart/held does its job. It still proves the fact for a helper whose step 1 alone compares places and for one that gives a mount point its directory's place; both are held by shipped cases and section 9 states the first. The design's own shape, a bind beneath allowed, was not measured against the place-only helper."},
  {"file": "crates/substrate-exec-gate/src/landlock.rs", "line": 774, "category": "test-gap", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "A8-F4: the ENOENT branch of the expansion is reachable by a concurrent run that writes the listed directory. Local soak, not a test: 300 starts under churn, shipped helper 300 x status 0, the helper without the branch 60 x status 125 'cannot open .../k/churn'. No suite case holds it and none can deterministically. A rule on a symbolic link's own inode showed no observable difference by reading; fstat failing is not reachable by a run, by reading."},
  {"file": "crates/substrate-host/tests/adversary5_exec_denied_round2.rs", "line": 1, "category": "security", "severity": "note", "verdict": "CONFIRMED", "origin": "pre-existing", "message": "A8-F5: no route to a denied file found beyond the stated limits. /proc/self/fd and /proc/<pid>/fd 126, linkat by descriptor into another listed directory fails, execveat with an empty path on a read and an O_PATH descriptor EACCES, an O_TMPFILE copy in the expanded directory EACCES by descriptor and linked, no mount and no user namespace inside a run, a denied file-system root holds under a bind of its part. A hard link left by an earlier start without the deny entry runs (N4); section 8.2's sentence that the run cannot make that link holds only for the run that has the entry."},
  {"file": "crates/substrate-host/tests/adversary4_exec_denied.rs", "line": 272, "category": "test-quality", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "A8-F6: the three reviewer assertions the implementor changed follow the design: the host mount case and the non-UTF-8 case are stricter, and the created-later case asserts sections 8.2 and 8.5 as written. Its second half accepts 126 or 127 per line where the commit message says 126 each."},
  {"file": "crates/substrate-host/tests/adversary5_exec_denied_round2.rs", "line": 1, "category": "test-infrastructure", "severity": "note", "verdict": "INFEASIBLE", "origin": "undecided", "message": "A8-F7: one run of the host binary through cargo test in the adversary's lane wrapper proved no fact (facts: [], both cases ABSENT); the same binary started directly in a delegated scope a minute later executed. Cause not established."}
]
```
