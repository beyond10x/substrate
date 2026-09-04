---
format: aep.planning-md/1
id: review-result:adversary-waveb-u1-pass-1
kind: review-result
status: active
title: Adversary pass 1, wave B unit u1 (lease cleanup projection)
relations:
- reviews: story:lease-cleanup-reads-exec-state-only
revision: 1
---
# Adversary pass 1 — wave B unit u1, `story:lease-cleanup-reads-exec-state-only`

Worktree `wt-01f593f6d86e`, branch `impl/lease-cleanup-bounded` at `d3d9dee`, base `0c858f0`.

```
unit: u1 story:lease-cleanup-reads-exec-state-only
verdict: green — no blocker; the headline hypothesis was measured and refuted
cases: executed 54→55 store, 151→151 daemon, red 0
origin: introduced 1 / pre-existing 3 / undecided 0
needs-coordinator: no
```

## The brief's main suspicion was tested and does not hold

The brief asked whether the 16 MiB resident ceiling does real work or whether any plausible
implementation sits under it. The adversary grafted the unit's guard **verbatim** onto the base
`execs.rs` in a scratch copy of the crate and ran it against the real pre-fix code:

| run | resident growth |
|---|---|
| 8/8 solo runs | **73,408,512 B** |
| full parallel base suite | 72,359,936 B |
| `--test-threads=1` | 73,408,512 B |

Ceiling 16,777,216 B — a **4.3–4.4× separation in every invocation**. The ceiling does real work and
the minimum-of-three does not wash the signal out.

## A case was written, ran red, and was withdrawn

`resident_growth_minimum_of_three_still_sees_the_whole_exec_load_the_guard_forbids` was red and
reproducible 5/5:

```
panicked at crates/substrate-store/src/tests.rs:3903:5:
the guard passes anything whose minimum-of-three resident growth is under 16777216 bytes; the
134217728-byte whole-exec load it was written to forbid measured [136384512, 6295552, 83763200],
minimum 6295552, so the guard is green for the defect too
```

**It is not a finding, because the theory it encoded is false.** It reconstructed the pre-fix load
from the test (`store.exec()` in a loop) rather than running the pre-fix `execs_for_workspace`, and
the two do not allocate alike. Withdrawn to scratch, not left in the tree.

## Case added and kept

`crates/substrate-store/src/tests.rs:3824` —
`workspace_lease_cleanup_reads_no_output_column_at_the_maximum_exec_count`. **Green.** Asserts the
acceptance clause the unit's own guards do not reach — 2048 rows (`MAX_CURRENT_EXECS`), every output
column holding a value the blob decoder refuses — and pins all four scope predicates of the
rewritten query.

Store lane 54 → 55, exit 0. Daemon lane 151 → 151, exit 0.

## Findings

**1 — three of the four scope predicates are unguarded.** `execs.rs:568`, warning, `pre-existing`.
Mutation probe on a scratch copy of HEAD: dropping `AND physically_absent = 0`, or neutering
`subject = ?2`, or neutering `deployment = ?1` each leaves the shipped suite at `ok. 54 passed`.
Control: `'$.state'` → `'$.kind'` is killed by the unit's own cases. Three of the four are
cross-tenant scoping. The same mutant survives at base (`ok. 52 passed`), so the gap predates the
rewrite. **The case added at `tests.rs:3824` kills all three.**

**2 — the stated reason for excluding `recovery.rs:133` is false.** note, `pre-existing`.
`mark_exec_physically_absent` reloads via `load_exec` inside its own transaction, and its `UPDATE`
sets only `resource_json`, `output_complete`, `physically_absent`. `RecoveryExec.stored` is read for
`.resource.id` and `.resource.workspace` and nothing else. The claim describes `leases.rs:461`, a
different site. `PROVISIONAL_RECOVERY_BATCH = 16`, so 32 MiB of blobs are loaded and never read —
bounded, hence outside this story, **but the enumeration is right by luck**.

**3 — a store error during workspace lease expiry destroys the workspace silently.**
`app/service.rs:698`, note, `INFEASIBLE`, `pre-existing`. `if let Ok(execs)` with no `else` falls
through to `destroy_workspace`, so `Accepted`/`Running`/`Unknown` execs are never signalled and no
refusal is named — invariant 3. **Nothing found:** `load_exec` is the only writer of those columns
and always binds a blob, so no reachable trigger was constructed. Identical shape at base.

**4 — the acceptance names a count the shipped guards do not reach.** `execs.rs:884`, note,
`introduced`. The acceptance says "over the maximum exec count"; the two shipped guards run at 3 and
64 rows. `MAX_CURRENT_EXECS` is 2048. Measured at 2048 the code satisfies it — the added case is
green there. Coverage only.

## Attacked, could not break

- **The column-poison case.** `UPDATE execs SET stdout = ?1, stderr = ?1` has no `WHERE`, so every
  row and both columns are poisoned; a regression reading `stderr` but not `stdout` is still caught.
- **The class enumeration.** `load_exec` (`execs.rs:683`) is the only reader of `stdout`/`stderr` in
  the crate. Its only collection-returning callers were `execs_for_workspace` and `recovery_execs`
  (bounded at 16). Ten other sites are single-row. No join, iterator or cross-crate path missed.
  **The conclusion holds; only finding 2's reason is wrong.**
- **`json_extract` vs the Rust enum.** All six `ExecState` variants are `rename_all = "lowercase"`
  plain strings with no `#[serde(other)]`, so an unknown state is a serde error, not a silent
  default. Base failed on the same input for the same reason.
- **The doc-comment arithmetic.** `MAX_CURRENT_EXECS = 2_048` and `MAX_IO_BYTES = 1_048_576` both
  check out; 4 GiB is a correct upper bound, and the cap really is a row cap on `execs`.
- **The daemon side.** The implementor's "no daemon case could have been red on the base" holds.
- **The coordinator's commit `d3d9dee`.** Correct and minimal; no API-surface golden drifted.

```findings
- file: crates/substrate-store/src/execs.rs
  line: 568
  category: mutant
  severity: warning
  verdict: needs-revision
  origin: pre-existing
  message: >-
    three of the four scope predicates on the rewritten execs_for_workspace query are unguarded —
    dropping "AND physically_absent = 0", or neutering "subject = ?2", or neutering "deployment = ?1"
    each leaves the shipped suite at "ok. 54 passed" on a scratch copy of HEAD, and the same mutant
    survives at base with 52 passed; the case added at tests.rs:3824 kills all three.
- file: crates/substrate-store/src/recovery.rs
  line: 133
  category: judgement
  severity: note
  verdict: needs-revision
  origin: pre-existing
  message: >-
    the stated reason for leaving this load_exec site out of the fix is false — mark_exec_physically_absent
    reloads inside its own transaction and its UPDATE sets only resource_json, output_complete and
    physically_absent, never the candidate's blobs; the claim describes leases.rs:461 instead, and the
    16 StoredExec blobs per recovery batch are loaded and never read.
- file: crates/substrate-daemon/src/app/service.rs
  line: 698
  category: judgement
  severity: note
  verdict: needs-revision
  origin: pre-existing
  message: >-
    "if let Ok(execs)" with no else falls through to destroy_workspace, so a store error during
    workspace lease expiry destroys the workspace without signalling its Accepted/Running/Unknown
    execs and without a named refusal (invariant 3) — but load_exec is the only writer of those
    columns and always binds a blob, so no reachable trigger was found.
- file: crates/substrate-store/src/execs.rs
  line: 884
  category: acceptance
  severity: note
  verdict: needs-revision
  origin: introduced
  message: >-
    the acceptance names a lease expiry "over the maximum exec count" while the two shipped guards run
    at 3 rows and 64 rows; MAX_CURRENT_EXECS is 2048 and is a per-subject row cap on execs, and the
    code does satisfy the acceptance at that count — the added case at tests.rs:3824 is green there.
```
