---
format: aep.planning-md/3
id: review-result:design-29-security-pass-1
kind: review-result
status: active
title: 'Security review pass 1: scoped workspace write on linux and output that keeps its tail'
relations:
- reviews: story:linux-adapter-serves-scoped-workspace-write
- reviews: story:output-keeps-tail-within-bound
revision: 1
---
unit: design 29 (b7d677ef, `story:linux-adapter-serves-scoped-workspace-write` and `story:output-keeps-tail-within-bound`); findings cover the working tree at 526656dc plus one new untracked test file
verdict: NEEDS-CHANGE
cases: executed 586→589, red 3
origin: introduced 2 / pre-existing 0 / undecided 1
wrote-outside-worktree: 7 paths, all in the assigned scratch directory
needs-coordinator: whether a consumer runs `Scoped` with a git submodule inside a writable subtree (finding 3 depends on it)

**1. Diff**

`git --no-pager diff --stat` is empty. `git status --short` shows one untracked file: `?? crates/substrate-host/tests/security_design29.rs`. It is a test file. I changed no implementation file.

**2. Cases added** (`crates/substrate-host/tests/security_design29.rs`). All three are red now. I ran them alone in a delegated scope with `cargo test -p b10x-substrate-host --test security_design29 -- --nocapture --test-threads=1`, before running the suite. Output, verbatim:

```
test a_root_git_status_does_not_run_a_command_a_scoped_run_chose ... security design 29 submodule redirect: executed
panicked at crates/substrate-host/tests/security_design29.rs:316:5:
a git status in the workspace root ran a command the confined run chose under Scoped(["out"]), through the submodule out/vendor, while the workspace's own .git stayed unchanged (run output: planted
)
test a_scoped_run_cannot_plant_a_repository_host_git_discovers_in_its_subtree ... security design 29 planted repository: executed
panicked at crates/substrate-host/tests/security_design29.rs:248:5:
assertion `left != right` failed: the confined run under Scoped(["out"]) made out/.git a repository whose core.fsmonitor it chose, and host git in out/ reads it (run output: planted
)
  left: "run-chosen-command"
 right: "run-chosen-command"
test the_changelog_states_the_two_limits_of_a_scoped_workspace_write ...
panicked at crates/substrate-host/tests/security_design29.rs:54:5:
the [Unreleased] CHANGELOG section does not state these limits of scoped workspace write: ["a pre-existing hard link stays writable", "a nested .git inside a writable subtree stays writable"]
test result: FAILED. 0 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.24s
EXIT=101
```

- `the_changelog_states_the_two_limits_of_a_scoped_workspace_write` (portable): the `[Unreleased]` section of the CHANGELOG must name both limits.
- `a_scoped_run_cannot_plant_a_repository_host_git_discovers_in_its_subtree` (delegated): a run under `Scoped(["out"])` must not be able to make `out/.git` a repository whose config host git reads. The host only reads the value back with `git config --get`, which runs nothing.
- `a_root_git_status_does_not_run_a_command_a_scoped_run_chose` (delegated): the workspace has a submodule at `out/vendor`. The run uses `sh` only to replace the submodule's `gitdir:` file with a repository that sets `core.fsmonitor = touch <marker>`. A host `git status` in the workspace root must not create the marker. The workspace's own `.git` is asserted byte-identical before and after.
- Host git in both delegated cases runs with a cleared environment, a private `HOME` and `GIT_CONFIG_NOSYSTEM`.

**3. Suite run**

The suite ran after the cases existed. Command: `systemd-run --user -p Delegate=yes --scope` (the setup from `delegated-lane.sh`), then `nice -n 19 cargo test -p b10x-substrate-host --locked --no-fail-fast -- --nocapture --test-threads=1`. Exit 101.

- Only `tests/security_design29.rs` failed (`0 passed; 3 failed`).
- Totals over 59 binaries: 586 + 3 passed/failed, 10 ignored.
- The 586 is this same run with my one file left out (no earlier `cases:` line was available).
- Absent, not executed, and not reported as passed: 21 oci and pod lane cases, because `SUBSTRATE_OCI_LANE` and `SUBSTRATE_POD_LANE` are unset.
- All 11 design-29 delegated cases from the implementor printed `executed`: linux scoped write, git, symlink, nested git, hard link, the three retention cases, and systemd local/linux scoped write and local tail.

**4. Findings**

| # | artifact | reason | verdict / origin / severity | what reaches it |
|---|---|---|---|---|
| 1 | `CHANGELOG.md` `[Unreleased]` (and `website/docs/guides/confinement-profiles.md`) | The commit message says "The CHANGELOG states both limits". Neither limit is in the CHANGELOG or on the site. A consumer relying on the `.git` protection is never told about them. | NEEDS-CHANGE / introduced / warning | every consumer reading the release notes |
| 2 | `docs/design/29-scoped-workspace-write-and-output-tail.md:116` | The design describes the nested-`.git` limit as something that already exists ("a vendored repository, a submodule checkout"). In fact the run can create one by itself in any writable subtree, with a config it chooses, so `core.fsmonitor` or `core.hooksPath` runs on the host the next time git runs inside that subtree (for example a shell prompt). Measured at `security_design29.rs:248`. | CONFIRMED / introduced / warning | any `Scoped` run, then an operator running git inside the subtree. I found no documented operator workflow that does this. |
| 3 | `docs/design/29-scoped-workspace-write-and-output-tail.md:101` ("`.git` needs no re-bind") | Same as finding 2, but here an ordinary `git status` in the workspace root runs the command the run chose. The root `.git` stays unchanged, so the `.git` protection is defeated. Measured at `security_design29.rs:317`. I also reproduced it with plain git 2.55 outside the sandbox (`git-probe.sh`). | INFEASIBLE / undecided / warning | needs a submodule inside a named writable subtree. I built that state myself and found no consumer that has it. |
| 4 | `crates/substrate-host/src/output_retention.rs:119` | Under `Tail` the ring first grows to `tail_cap + chunk` (`extend`) and only then trims (`drain`, :121). A `VecDeque` can therefore hold about 2×B per stream, while design § 6 says "Memory is at most `B` per stream". The kept bytes are still within B. Suggested fix: trim before extending, with `with_capacity(tail_cap)`. Not measured, because `RetentionBuffer` is `pub(crate)` and a test outside the crate cannot reach it. | CONFIRMED (code reading only, not run) / introduced / note | every `Tail`/`HeadAndTail` run longer than B |

Answer to brief point 3: the two observed limits differ in who can create them.

- **Hard link:** a run cannot create one. Linking across the bind mounts fails with `EXDEV` (A2 shows this). It needs a host actor, which the design puts outside the threat model. This is an acceptable limit, but it should be documented (finding 1).
- **Nested `.git`:** a run can create one by itself (findings 2 and 3). With a submodule under a writable subtree it defeats the purpose of the `.git` protection. At minimum the design and CHANGELOG must say that `Scoped` protects only the root `.git` and gives no protection against host code execution through repositories inside writable subtrees. Refusing such a profile would be a design change and is not something I can decide.

Not tested: a root `.git` that is a `gitdir:` file pointing into a writable subtree, and `core.hooksPath` set to a directory inside the worktree (husky). Both need the operator's own layout, and a run cannot create either.

**5. Checked and not faulted**

- **Point 1 (named refusals):**
  - local, macos, oci, pod, and systemd around local refuse with `refuse_workspace_access` at `check` (`mod.rs:813`).
  - linux on a host without the fact refuses with `check_snapshot` (`linux.rs:362`), and the driver checks the fact again at start (`process.rs:1934`).
  - pod refuses tail retention (`pod.rs:767`).
  - `AdmittedProfile.profile` is private, so a profile cannot be changed after admission.
  - systemd around linux admits again through linux `check` (`systemd.rs:659`).
- **Point 2 (descriptor-bound TOCTOU):** at step 2 (cited `file:line`) plus A6 (static symlink) at step 4. The race window itself has no step-4 case.
  - Admission walks the subtree with openat2 beneath/no-symlink (`process.rs:1949`).
  - Start walks every component again from a copy of the pinned root with `O_PATH|O_NOFOLLOW|O_DIRECTORY` (`:2009`).
  - Bubblewrap binds the descriptors (`:323`). A path swapped after pinning only changes where the pinned object is mounted, never which object.
- **Point 4 (byte bound):** head ≤ head_cap and tail ≤ tail_cap.
  - `observed_bytes` is set only on EOF and never after a read failure.
  - The linux `Head` marker path corrects `head_bytes` (`process.rs:3682`).
  - Sessions are forced to `Head`.
  - pod truncates to the bound and reports `observed_bytes: None`.
- **Point 5 (applied record):** the driver's applied `workspace_access` is the input that `bind_args` enforces, and `KeptOutput.retention` is the mode actually used (`Head` when a sender is attached). The confinement audit record makes no per-field claim.
- **Point 6 (wire and CLI unchanged):** `git diff b7d677ef^ b7d677ef -- crates/substrate-wire contracts crates/substrate-cli xtask spec` is empty. `start_exec_retaining` is `pub(crate)`, the `Driver` trait is unchanged, and the public-items additions match design § 8.

**6. Paths written outside the worktree** (the assigned scratch directory)

- `<scratch>/sec-29/git-probe.sh`
- `<scratch>/sec-29/git-probe/` (throwaway repositories and marker)
- `<scratch>/sec-29/run-review.sh`
- `<scratch>/sec-29/run-suite.sh`
- `<scratch>/sec-29/case-alone.log`
- `<scratch>/sec-29/suite.log`
- `<scratch>/sec-29/summary.txt`

The test temp directories were created under `TMPDIR` by `tempfile` and removed by it. The worktree session lease `sec29-a3687f97` was acquired and released.

**7. Findings block**

```findings
- file: CHANGELOG.md
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: the commit says the CHANGELOG states the nested-.git and pre-existing-hard-link limits of scoped write, but neither the Unreleased section nor the site guide states them (security_design29.rs:54)
- file: docs/design/29-scoped-workspace-write-and-output-tail.md
  line: 116
  category: acceptance
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: a Scoped run can itself create a nested repository in a writable subtree with a config it chose, which host git in that subtree reads, so the nested-.git limit is not only about pre-existing repositories (security_design29.rs:248)
- file: docs/design/29-scoped-workspace-write-and-output-tail.md
  line: 101
  category: integrity
  severity: warning
  verdict: INFEASIBLE
  origin: undecided
  message: with a submodule inside a writable subtree, the run replaces its gitdir file and a root-level git status runs the run's core.fsmonitor on the host while the root .git stays unchanged (security_design29.rs:317); no consumer was shown to have that layout
- file: crates/substrate-host/src/output_retention.rs
  line: 119
  category: judgement
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: the tail ring extends before it trims, so its allocation can reach about twice the bound per stream, against design section 6's "memory is at most B per stream"; the kept bytes stay within the bound
```
