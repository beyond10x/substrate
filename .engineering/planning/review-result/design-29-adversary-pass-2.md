---
format: aep.planning-md/3
id: review-result:design-29-adversary-pass-2
kind: review-result
status: active
title: 'Adversary pass 2: design 29 retention bounds, admission spellings, scoped write conformance'
relations:
- reviews: story:linux-adapter-serves-scoped-workspace-write
- reviews: story:output-keeps-tail-within-bound
revision: 1
---
unit: design 29, commit b7d677ef (tree 526656dc), attacked in `substrate-scoped-write` while another session had uncommitted edits in the tree
verdict: CONFIRMED (2 red cases; neither is a blocker)
cases: executed 599→613, red 2
origin: introduced 2 / pre-existing 0 / undecided 0
wrote-outside-worktree: 7 paths (part 6)
needs-coordinator: another session is editing implementation files in this tree while the pass runs (part 1)

**1. Diff stat**

My only change is the new untracked file `crates/substrate-host/tests/adversary_design29.rs` (952 lines), so it does not show in `git --no-pager diff --stat`. That command shows only other sessions' uncommitted work, none of it mine: `CHANGELOG.md`, `confinement/linux.rs`, `fs.rs`, `lib.rs`, `output_retention.rs`, `process.rs`, `tests/ess_confinement_linux.rs`, `docs/design/29-…md`, `scripts/delegated-lane.sh`, `website/docs/guides/confinement-profiles.md`, plus the untracked `src/submodules.rs` and `tests/security_design29.rs`. Neither red case touches their hunks. Both cited lines are the same at HEAD (process.rs:2463/2523, design:264) and in the working tree (process.rs:2482/2542, design:295).

One more thing I changed in the tree: I deleted three core dumps that my first suite run left in `crates/substrate-host/` (`core.2462146`, `core.2462156`, `core.2462159`, 51 MB each). They came from the secrets-test children of my run, which aborted because of the long `TMPDIR` I had set.

**2. Cases added** (`adversary_design29.rs`; every expected value comes from oracles in the file, not from the code under test)

| case | asserts | now |
|---|---|---|
| `adv_local_retention_one_under_the_bound` / `_exactly_the_bound` / `_one_over_the_bound` | `local` at 1 MiB − 1, B, B + 1. Each mode meets each size on both streams, with stdout and stderr in different modes in the same run. Checks kept bytes, the head/tail split, `observed_bytes`, the omitted count and `*_truncated`. Head is checked against the base algorithm. | green |
| `adv_local_default_profile_is_head_on_both_streams` | the default profile gives the same output as b7d677ef~1 | green |
| `adv_workspace_access_admission_spellings_on_local` | `./.git`, `.git/`, `a/../.git`, `/.git`, a backslash, a NUL, `out,out`, `.git,.git`, `a/.git,a/.git`, `.git,.git/hooks` → invalid (422), shape checked first. `.git`, `.GIT`, `.gIt/objects`, `vendor/.Git` → `confinement.writable-subtree-git` (422). `.github`, `.git.d,x.git`, `.gitmodules`, `git` → pass both rules (501 on `local`). | green |
| `adv_linux_retention_bound_one` | bound 1 at sizes 0/1/2/9000; HeadAndTail is a one-byte tail | green |
| `adv_linux_retention_odd_bound_at_the_marker_length` | bound 31 at 30/31/32; Head over the bound is the marker alone, as in the base | green |
| `adv_linux_retention_odd_bound_4097` | 4096/4097/4098/30001; odd split 2048 + 2049 | green |
| `adv_linux_workspace_access_admission_spellings` | the same table on `linux`; what passes both rules is admitted | green |
| `adv_linux_subtree_refusals_at_start` | missing, `a/missing`, `missing/deeper` → absent. `file`, `file/x` → not-directory. Leaf symlink, intermediate symlink, absolute symlink, dangling symlink → symlink. All 422 and audited; host unchanged. | green |
| `adv_linux_scoped_write_conformance` | `Scoped(["a/b","c"])`, 28 attempts: writes outside, append/truncate/chmod/touch/rm, mkdir/mkfifo/symlink, renames out of a subtree, renames of the subtree mounts, a hard link out, a symlink made in the run, host-made links (relative, `/workspace/d`, `..`, absolute host path), and `.git`. Only the two declared writes land. Host workspace, the outside directory and the links in `c` are byte-, mode- and mtime-unchanged. | green |
| `adv_linux_read_only_workspace_conformance` | `ReadOnly`: all 28 attempts refused; host unchanged | green |
| `adv_linux_output_while_running_is_the_current_snapshot` | the design says `output()` mid-run returns the current snapshot; checked with a host-file handshake, no timing guess | **red** |
| `adv_linux_render_refuses_a_subtree_as_start_does` | `render` and `start` give the same refusal for missing, file and symlink subtrees; the rendered argv for `["out"]` is `--ro-bind-fd … /workspace` then `--bind-fd … /workspace/out` | **red** |

Red output from each case run alone, before the suite (verbatim):
```
test adv_linux_render_refuses_a_subtree_as_start_does ...
thread 'adv_linux_render_refuses_a_subtree_as_start_does' (2346444) panicked at crates/substrate-host/tests/adversary_design29.rs:892:5:
adversary design29 linux render parity: render and start disagree:
missing: start exec.workspace-subtree-absent 422, render exec.workspace-subtree-changed 422 (exec.workspace-subtree-changed: Writable workspace subtree missing changed before it could be pinned.)
file: start exec.workspace-subtree-not-directory 422, render exec.workspace-subtree-changed 422 (exec.workspace-subtree-changed: Writable workspace subtree file changed before it could be pinned.)
lnk: start exec.workspace-subtree-symlink 422, render exec.workspace-subtree-changed 422 (exec.workspace-subtree-changed: Writable workspace subtree lnk changed before it could be pinned.)
FAILED
```
```
test adv_linux_output_while_running_is_the_current_snapshot ...
thread 'adv_linux_output_while_running_is_the_current_snapshot' (2359744) panicked at crates/substrate-host/tests/adversary_design29.rs:864:5:
assertion `left == right` failed: adversary design29 linux output while running: while running, the bytes already written; record KeptOutput { retention: Tail, observed_bytes: None, head_bytes: 0, tail_bytes: 0 }
  left: []
 right: [97, 98, 99]
FAILED
```

How the delegated cases ran: inside `systemd-run --user -p Delegate=yes --scope`, with the same cgroup setup as `scripts/delegated-lane.sh` and `SUBSTRATE_VECTORS_CGROUP_ROOT` set, `--test-threads=1`. Every delegated case in my file printed `executed`; none of them was absent.

**3. Suite run**

Command: `delegated.sh --no-fail-fast -- --nocapture --test-threads=1`, which is `nice -n 19 cargo test -p b10x-substrate-host --locked` in the delegated scope with the required env. Result: 60 binaries, 609 passed, 4 failed, 10 ignored, `EXIT=101`. The failed binaries were `adversary_design29` (12 passed, my 2 red) and `security_design29` (1 passed, 2 failed, the reviewer's). `executed 599→613` comes from this one run: 613 executed (609 + 4), minus my binary's 14 = 599.

- The oci and pod lanes printed ABSENT because `SUBSTRATE_OCI_LANE` is unset and there is no cluster. They were outside the brief.
- An earlier run with `TMPDIR` set to my scratch directory failed 5 lib tests (`SUN_LEN` on a socket bind, SIGABRT in the secrets children). My long path caused that; the lib passes with the default `TMPDIR`. That run's log is `suite.log`.
- After the lint fixes, clippy (pedantic, as the workspace sets it) reports nothing in my file and `rustfmt --check` is clean.

**4. Findings**

| # | file:line | verdict / origin | what was measured | what reaches it |
|---|---|---|---|---|
| 1 | `crates/substrate-host/src/process.rs:2523` (HEAD) | CONFIRMED / introduced | `ProcessRuntime::render` calls `prepare_workspace_access` without `admit_writable_subtrees` (start calls it at :2336). So a dry run names `exec.workspace-subtree-changed` where start names `-absent`, `-not-directory` or `-symlink`. This breaks the doc comment at :2463 ("refused exactly as a start pins and refuses them"). Because the render path has no `RESOLVE_NO_XDEV` walk, it would also render a subtree that crosses a mount, which start refuses; I did not test that (no unprivileged mount). Fix: call `Self::admit_writable_subtrees(&pinned, &input.workspace_access)?` before :2523. | Public `ConfinementAdapter::render` (also `systemd` around `linux`) with a non-default `workspace_access`. Before this commit `linux` always sent `ReadWrite`, so nothing reached this path. No caller in the repo passes `Scoped`; `confine run --dry-run` has no flag for it. |
| 2 | `docs/design/29-scoped-workspace-write-and-output-tail.md:264` | CONFIRMED / introduced | The design says "`output()` while the run is running returns the current snapshot". On `linux`, `output()` mid-run returns empty bytes with `{head 0, tail 0, observed None}`, because `run_child` sets `observation.stdout` only after the drains end (process.rs:3044, the same at base :2990). `local` does return bytes mid-run. Fix: either scope the sentence to the adapters that read their own pipes, or publish the drain's buffer. | Any consumer polling `output()` on a running `linux` run. The behaviour is the same as before this commit; the new design text claims otherwise. |

**5. Attacked, could not break**
- Head output is byte-identical to the base on `local` (B ± 1) and on `linux` (bounds 1, 31, 4097), marker included.
- `observed_bytes`, the head/tail split, the omitted count and `*_truncated` are exact at every boundary tried. stdout and stderr keep separate modes in one run.
- Shape is checked before `.git` for every spelling tried. `.GIT` is refused 422 on a case-sensitive fs, which is the design's stated choice; I found no non-ASCII spelling that ext4 casefold maps to `.git`.
- `file/x` gives the correct not-directory refusal (the walk stops at `file`).
- Scoped write conformance holds:
  - nothing outside `a/b` and `c` can be written, renamed, linked, chmodded or touched;
  - the subtree mounts cannot be renamed;
  - symlinks made in the run or on the host do not make their targets writable;
  - the host is unchanged.
- `ReadOnly` holds.

Notes for the coordinator, not findings:
- The uncommitted work in progress adds the registered-submodule refusal in `start_inner` only, so it would widen finding 1's render/start gap.
- The uncommitted `VecDeque::with_capacity(tail_cap)` allocates the whole bound when the stream opens. That contradicts design § 6's "a quiet stream costs nothing". It is not committed, so I did not test it.

**6. Paths written outside the worktree**
- `<scratch>/adv-29/build.sh`
- `<scratch>/adv-29/delegated.sh`
- `<scratch>/adv-29/suite.log`
- `<scratch>/adv-29/suite2.log`
- `<scratch>/adv-29/final.log`
- `<scratch>/adv-29/clippy.log`
- `<scratch>/adv-29/tmp/` (empty)
- Test tempdirs in `~/.cache/claude-tmp` were removed when the tests dropped them.

**7.**
```findings
- file: crates/substrate-host/src/process.rs
  line: 2523
  category: contract-drift
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: ProcessRuntime::render skips admit_writable_subtrees, so a dry run of a scoped linux profile names exec.workspace-subtree-changed where start names -absent, -not-directory or -symlink, contrary to render's own doc comment at :2463
- file: docs/design/29-scoped-workspace-write-and-output-tail.md
  line: 264
  category: contract-drift
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the design says output() while running returns the current snapshot, but on linux it returns no bytes until the run ends
```
