---
format: aep.planning-md/1
id: review-result:adversary-u4-pass-1
kind: review-result
status: active
title: Adversary pass 1, unit u4 (disable-userns)
relations:
- reviews: story:confined-processes-cannot-nest-user-namespaces
revision: 1
---
# Adversary pass 1 — unit u4, `story:confined-processes-cannot-nest-user-namespaces`

Agent type `adp:adversary`. Worktree `wt-5698af5e5dcf`, branch `impl/disable-userns`,
HEAD `a941dcd`, base `617bbed`. Report as returned.

```
unit: u4 — story:confined-processes-cannot-nest-user-namespaces
verdict: red
cases: executed 84→88, red 3
origin: introduced 4, pre-existing 0, undecided 0
wrote-outside-worktree: 5 paths
needs-coordinator: no
```

## Cases added

`crates/substrate-host/tests/user_namespace_posture_adversarial.rs` (untracked; no implementation
file touched, `seccomp.rs` untouched).

| case | asserts | result |
|---|---|---|
| `the_re_expressed_posture_rule_agrees_with_the_shipped_case_on_the_unmodified_tree` | control: the re-expressed rule is green on the untouched tree | green |
| `the_posture_check_catches_an_exec_argv_that_stopped_splicing_the_constant` | with `command.args(USER_NAMESPACE_ARGV);` deleted, the check reports a violation | **red** |
| `the_posture_check_catches_an_eighth_sandbox_that_spells_the_user_namespace_differently` | with an `--unshare-all` sandbox added, the check reports a violation | **red** |
| `a_backend_that_cannot_disable_nested_user_namespaces_withholds_every_exec_fact` | with a bwrap that refuses both options, no `exec.` key is published | **red** |

Run of these cases alone, `cargo test -p b10x-substrate-host --release --locked --test user_namespace_posture_adversarial`, exit 101:

```
running 4 tests
test the_posture_check_catches_an_eighth_sandbox_that_spells_the_user_namespace_differently ... FAILED
test the_posture_check_catches_an_exec_argv_that_stopped_splicing_the_constant ... FAILED
test the_re_expressed_posture_rule_agrees_with_the_shipped_case_on_the_unmodified_tree ... ok
test a_backend_that_cannot_disable_nested_user_namespaces_withholds_every_exec_fact ... FAILED

---- the_posture_check_catches_an_eighth_sandbox_that_spells_the_user_namespace_differently stdout ----
panicked at crates/substrate-host/tests/user_namespace_posture_adversarial.rs:179:5:
a sandbox spelled --unshare-all creates a user namespace with nesting left open and the class check
is green, so `an eighth sandbox gets the posture or gets a red case` (process.rs:45-46) does not
hold for --unshare-all, --unshare-user-try or --userns

---- the_posture_check_catches_an_exec_argv_that_stopped_splicing_the_constant stdout ----
panicked at crates/substrate-host/tests/user_namespace_posture_adversarial.rs:132:5:
the exec argv no longer carries --unshare-user or --disable-userns and the class check is still
green: it proves the constant is well-formed, not that any sandbox uses it, so review finding 6's
exact shape — six argv lists right and the exec one wrong — passes it

---- a_backend_that_cannot_disable_nested_user_namespaces_withholds_every_exec_fact stdout ----
panicked at crates/substrate-host/tests/user_namespace_posture_adversarial.rs:239:5:
assertion `left == right` failed: probe.rs:369 says such a backend leaves `every exec fact
**absent**`; these are still published, so a client reading the capability document sees exec facts
served by a host whose confinement floor was never proved
  left: ["exec.max-current", "exec.output-limit-bytes"]
 right: []

test result: FAILED. 1 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out
```

Package gate after the cases existed: **exit 101**, 84 → 88 executed. Delegated host lane: exit 101,
same result; the unit's own acceptance passed there
(`process::tests::a_confined_process_cannot_nest_a_user_namespace ... ok`).

## Findings

| # | file:line | verdict | origin | measured | reaches it |
|---|---|---|---|---|---|
| F1 | `crates/substrate-host/src/process.rs:45` | needs-revision | introduced | test:132, exit 101 — the class check stays green with the exec argv's only splice deleted | nothing today; the claim is about future edits. Constructed corpus |
| F2 | `crates/substrate-host/src/process.rs:46` | needs-revision | introduced | test:179, exit 101 — the check stays green with an `--unshare-all` sandbox added | nothing today; no in-tree site uses that spelling. Constructed corpus |
| F3 | `crates/substrate-host/src/probe.rs:369` | needs-revision | introduced | test:239, exit 101 — `["exec.max-current","exec.output-limit-bytes"]` still published | any host whose bwrap predates `--disable-userns`. Real, but no exec is admitted: `operations.rs:502` gates on `exec_namespaces` |
| F4 | `docs/design/15-docker-driver-entry-gate.md:152` | needs-revision | introduced | read, not run — the floor table has no row for nested user namespaces | a second driver admitted through that gate. Judgement |
| F5 | `crates/substrate-host/src/probe.rs:371` | needs-revision | introduced | read, not run — no case pins `--assert-userns-disabled` | every run of the shipped probe case. Judgement |

**F1 — the class check does not check the class.** `process.rs:45-46` claims the check makes an
eighth sandbox get the posture or get a red case. The check reads the crate's sources for
`"--unshare-user"` and proves the *constant* is written once with both halves. It never reads
whether any argv **uses** it. Delete `process.rs:1834` `command.args(USER_NAMESPACE_ARGV);` and the
tree is back in review finding 6's exact shape — six argv lists carrying the posture, the exec one
not — and the check is green. On the portable lane nothing else covers it: the acceptance case is
absent without `SUBSTRATE_VECTORS_CGROUP_ROOT`, and the probe case only exercises `probe.rs`. The
same blindness covers `pty.rs`'s new comment about splicing at every use site. Fix not applied:
assert on the built argv, not the source text — a recording `bwrap` stub in `config.bubblewrap`
plus one exec through the public `HostDriver`, asserting the captured argv carries both options.
That runs on the portable lane and pins all seven sites.

**F2 — three other spellings evade the grep.** `bwrap --help` on 0.11.2 lists `--unshare-all`
(includes the user namespace), `--unshare-user-try` and `--userns FD`. None contains the needle —
`--unshare-user-try` does not, because the needle carries the closing quote. Also
`std::fs::read_dir` is non-recursive, so a future `src/sandbox/mod.rs` is invisible regardless of
spelling.

**F3 — "every exec fact absent" is not what the snapshot does.** `probe.rs:366-369`, `probe.rs:876`
and the commit message all state such a backend leaves *every* exec fact absent. Measured through
`HostDriver::open` → `machine()`, `exec.max-current` and `exec.output-limit-bytes` are still
published. The behaviour reproduces at the base (`617bbed:probe.rs:126-127`), so the *code* half is
pre-existing; the **contradiction** is new, because this diff wrote the sentence. Cheapest correct
fix is the sentence.

**F4 — a new floor clause three floor documents do not name.** AGENTS.md's enforced isolation set
(`:78-84`), the docker entry gate's clause-by-clause table, and
`docs/design/10-destination-bound-egress.md:34` (which still describes the posture as
`--unshare-net` with `--unshare-user/ipc/pid/uts`). The design-10 citation was already stale at
base, so that half is pre-existing.

**F5 — the one line the story's Notes name is a free mutant.** The shipped probe case's stub refuses
`--disable-userns` *and* `--assert-userns-disabled`; since the former arrives via
`USER_NAMESPACE_ARGV`, deleting `.arg("--assert-userns-disabled")` at `probe.rs:371` leaves the
case green. Not reproduced as a red case: `probe_bubblewrap` is private and the mutation is
behavioural, which would need an implementation edit.

## Attacked and could not break

- **The flag itself works.** Inside the exact exec posture, `unshare -U` fails
  (`unshare failed: No space left on device`, exit 1), `unshare(CLONE_NEWUSER)` returns ENOSPC, and
  a direct `syscall(56, CLONE_NEWUSER|SIGCHLD)` returns ENOSPC. There is no open route left for
  `seccomp.rs` to close, so this is not a pre-existing seccomp finding.
- **Re-raising the limit from inside fails.** Empty capability set;
  `/proc/sys/user/max_user_namespaces` write is EACCES and nesting stays refused after.
- **All seven argv sites are the seven.** Repo-wide grep for `--unshare-*` finds bubblewrap argv
  construction only in this crate, at the seven named sites.
- **No conditional strips the posture from the exec argv.** `command.args(USER_NAMESPACE_ARGV)` is
  unconditional in `ProcessRuntime::command`; no profile, network mode, capsule, aperture, scratch
  or terminal branch removes it.
- **The exec facts do not admit anything they should not.** `operations.rs:502` and
  `pipe_confinement_available` both require `exec_namespaces`, so F3 leaks no admission.
- **`--disable-userns` does not break the egress forwarder's `setns` handback or the pty probe** —
  73 lib cases and all 6 pty acceptance cases pass on the delegated lane.
- **Not run, deliberately:** the daemon / SDK / MCP packages and the full `scripts/delegated-lane.sh`
  — disk at 99%, package gate only.

```findings
- file: crates/substrate-host/src/process.rs
  line: 45
  category: mutant
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: >-
    The class check proves the constant is well-formed, not that any argv splices it, so review
    finding 6's exact shape passes it. Delete the single line process.rs:1834
    `command.args(USER_NAMESPACE_ARGV);` and the exec argv carries neither --unshare-user nor
    --disable-userns, a confined process can nest a user namespace again, and
    the_user_namespace_posture_is_written_in_exactly_one_place is still green. The portable lane
    covers nothing else: the acceptance case is absent without SUBSTRATE_VECTORS_CGROUP_ROOT and
    the probe case only exercises probe.rs. Measured at tests/user_namespace_posture_adversarial.rs:132, exit 101.
- file: crates/substrate-host/src/process.rs
  line: 46
  category: contract-drift
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: >-
    "An eighth sandbox gets the posture or gets a red case" is false for --unshare-all,
    --unshare-user-try and --userns, and for any source file in a subdirectory of src/. Add a
    sandbox spelled .args(["--unshare-all", ...]) with no --disable-userns and the child gets a
    user namespace with nesting open while the check reports exactly one site, green. The needle
    carries a closing quote so --unshare-user-try does not match, and read_dir is non-recursive so
    a future src/sandbox/mod.rs is not read at all. Measured at
    tests/user_namespace_posture_adversarial.rs:179, exit 101.
- file: crates/substrate-host/src/probe.rs
  line: 369
  category: contract-drift
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: >-
    The new comment, the new test doc at :876 and the commit message all say such a backend leaves
    every exec fact absent; two are still published. HostDriver::open with a bwrap that exits
    non-zero on --disable-userns and --assert-userns-disabled still serializes exec.max-current and
    exec.output-limit-bytes. The ungated lines reproduce at base 617bbed probe.rs:126-127, so only
    the claim is new. No admission leaks: operations.rs:502 gates on exec.namespaces. Measured at
    tests/user_namespace_posture_adversarial.rs:239, exit 101.
- file: docs/design/15-docker-driver-entry-gate.md
  line: 152
  category: judgement
  severity: note
  verdict: needs-revision
  origin: introduced
  message: >-
    A new floor clause was added without a row in the clause-by-clause floor table, without an entry
    in AGENTS.md's enforced isolation set (:78-84), and docs/design/10-destination-bound-egress.md:34
    still describes the posture as --unshare-user/ipc/pid/uts. A container driver walked through
    this table's floor clauses can be admitted while leaving nested user namespaces open, because
    no row requires them closed. The design-10 citation was already stale at base, so that half is
    pre-existing. Read, not run.
- file: crates/substrate-host/src/probe.rs
  line: 371
  category: mutant
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: >-
    The shipped probe case's stub refuses both options, so the --assert-userns-disabled line the
    story's Notes name specifically is pinned by nothing. Deleting .arg("--assert-userns-disabled")
    leaves a_backend_that_cannot_disable_nested_user_namespaces_withholds_the_exec_floor green,
    because the stub still refuses --disable-userns, which arrives via USER_NAMESPACE_ARGV. Not
    reproduced as a red case: probe_bubblewrap is private and the mutation is behavioural, which
    would require an implementation edit. Read, not run.
```

## Coordinator note on this record

The adversary returned its findings block keyed `summary` / `failure_scenario`, because the brief
template this wave dispatched with named those fields. The store's schema is
`{file, line, category, severity, verdict, origin, message}`, and its severity vocabulary is
`blocker | warning | note`, not `blocker | major | minor`. The block above is the same five findings
with `summary` and `failure_scenario` merged into `message`, and severity re-mapped: the agent's
`major` (F1) to `blocker`, its `minor` to `warning` for F2, F3 and F5, and to `note` for F4, which
is a documentation judgement it did not run. No finding was added, dropped or reworded beyond
that. The brief template was corrected before the next adversary was
dispatched. The prose above the block is the agent's own text unedited.
