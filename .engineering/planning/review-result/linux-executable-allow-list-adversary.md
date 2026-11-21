---
format: aep.planning-md/3
id: review-result:linux-executable-allow-list-adversary
kind: review-result
status: active
title: 'Adversary pass, three rounds: the executable allow-list on linux'
tags:
- adversary
relations:
- reviews: story:linux-executable-allow-list
revision: 1
---
# Adversary pass on `story:linux-executable-allow-list`, three rounds

Reviewer: the `aep:adversary` role, run three times on 2026-11-21, each on the commit before it.
Verdicts: round 1 NEEDS-CHANGE, round 2 NEEDS-CHANGE, round 3 no blocker.

## Rounds 1 and 2, summarized by the coordinating session

Round 1, on `4b5208922`:

- F1, blocker: the run's environment reached `/usr/bin/env` and the helper before the ruleset; an
  `LD_PRELOAD` object in the workspace executed an unlisted program (stdout `0,0,0` against the
  control's `0,126,126`). Fixed in `767fbea57` by design 35's D28.
- F2: an entry whose view path exceeds 4095 bytes was dispatched and ended 125 by the helper.
  Fixed in `767fbea57`: refused before dispatch `exec.sandbox-path-unrepresentable`.
- F3 to F7: five guards no shipped test held. The pass wrote cases for three; unit cases for the
  other two came in `767fbea57`.

Round 2, on `767fbea57`:

- F8, blocker: a read-only file declared at `/etc/ld.so.preload` ran an object through the
  dynamically linked helper's loader before the ruleset (stdout `LEAKED`). Fixed in `1d961b30f`
  by D29: a static helper, refused at the open otherwise.
- Held: D28's operand parsing and operand-only environment (five mutants caught), the path-length
  refusal, the `..` refusal, the non-UTF-8 mount table.

Round 3's points G1 to G3 below were fixed in `a84b27edf`: unit cases for the two ELF guards, and
a declared mount at the helper's path refused before dispatch.

## Round 3, the reviewer's report

Verbatim, except that machine-local paths are replaced by `<scratch>` and `<worktree>`.

The findings block is rewritten into the store's fields; the reviewer's ids are in each message.

Third-pass adversary review complete. Report follows.

---

**Adversary — third pass (D29)**
- Story: `story:linux-executable-allow-list` (design 35), substrate
- Commit under attack: `1d961b30f` (fix of F8: static helper, first in the view) on top of `767fbea57`
- Branch: `story/linux-executable-allow-list-impl`
- Worktree: `<worktree>`
- Base: `main` at `3d603e63f`; lease `adversary3-ef94b977` released

```
git --no-pager diff --stat   (mine only)
 crates/substrate-host/tests/adversary_exec_lists.rs | 66 ++++++++++++++++++++++
 1 file changed, 66 insertions(+)
# Also dirty in the shared worktree, NOT mine — left untouched:
#   crates/substrate-exec-gate/tests/gate.rs, crates/substrate-exec-gate/README.md,
#   scripts/delegated-lane.sh  (implementor's live edits; coordinator said do not touch gate.rs)
#   crates/substrate-host/tests/security3_exec_gate.rs  (the security reviewer's, untracked)
```

## Coordinator ask 1 — the case still proves what mine proved: CONFIRMED

The implementor tightened `adversary2_a_read_only_ld_so_preload_runs_before_the_ruleset` (`adversary_exec_lists.rs:117`) so a refusal is no longer a pass: the profile must be admitted, the start dispatched, and the run must print `0\n126\n126\n`. Built with the **static musl helper** the lane now names, it passes:

```
lane-static: static gate   Type: DYN (Position-Independent Executable file)   # static-PIE, no PT_INTERP
test adversary2_a_read_only_ld_so_preload_runs_before_the_ruleset ... adversary2 exec list ld.so.preload: executed
test result: ok. 1 passed; 0 failed; 8 filtered out
```

The static helper has no loader, so a declared `/etc/ld.so.preload` runs nothing before the ruleset; the unlisted `echo` is refused (126). The case stays a live regression guard: it goes red (`"LEAKED\n"`) if the helper ever reverts to a dynamic build, and panics if admission starts refusing the mount. The F8 escape is fixed.

## New case written — the helper mount cannot be shadowed: HELD

`crates/substrate-host/tests/adversary_exec_lists.rs:698` — `adversary3_a_read_only_file_cannot_shadow_the_helper_mount`. `/.substrate-exec-gate` is **not** in `collides_with_owned_mount`, so a profile may declare a read-only file there; the only thing keeping it from replacing the trusted helper is bubblewrap bind order (the gate bind at `process.rs:3057` after the read-only-file binds at `process.rs:3033`). The declared file is a copy of `true`; were it the executed helper it would exit 0 with empty stdout. Run alone, first with the strict assertion (red, for evidence), then with the security-property assertion (green):

```
# evidence run (strict assertion):
  left: ("", Some(125))   right: ("0\n126\n126\n", Some(0))
# final run (asserts the shadow never becomes the helper):
adversary3 exec list helper shadow: stdout "" code Some(125)
  stderr "substrate-exec-gate: cannot establish the place of /.substrate-exec-gate: mount-root-deleted"
test result: ok. 1 passed
```

The shadow `true` never ran (that would be `("", Some(0))`). The real helper ran and **fail-closed** with exit 125 `mount-root-deleted` (it trips on its own mount's deleted memfd root). No escape. Full lane suite on the static helper: **10 passed, 0 failed**.

## Mutants on the new D29 code (helper crate, scratch copies)

```
pt-interp-not-detected          CAUGHT  linkage_is_the_presence_of_pt_interp
foreign-guard-removed           CAUGHT  an_image_of_another_machine_and_a_script_are_foreign
phnum-zero-accepted             CAUGHT  a_truncated_image_is_unreadable
phnum-xnum-accepted             SURVIVED  (no test)
entry-size-check-removed        SURVIVED  (no test)
data-endian-check-dropped       SURVIVED  (equivalent mutant — Some(found)==native already compares data)
```

## Findings

```findings
- file: crates/substrate-exec-gate/src/lib.rs
  line: 470
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: G1. The PN_XNUM guard of program_header_types is covered by no test; with the guard removed the helper's unit suite is still 36 passed, 0 failed.
- file: crates/substrate-exec-gate/src/lib.rs
  line: 470
  category: mutant
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: G2. The guard that e_phentsize is not below the program header size is covered by no test; with the guard removed the helper's unit suite is still 36 passed, 0 failed.
- file: crates/substrate-wire/src/lib.rs
  line: 4782
  category: boundary
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: G3. A read-only file declared at /.substrate-exec-gate is admitted and dispatched, and the helper ends the run 125 (mount-root-deleted) instead of the driver refusing before dispatch. The shadow never becomes the helper, so the run fails closed; case adversary3_a_read_only_file_cannot_shadow_the_helper_mount in crates/substrate-host/tests/adversary_exec_lists.rs.
```

## What I attacked that held

- **The ELF linkage test cannot false-negative for a native image.** The test reads class/data/machine, then every `e_phnum` program header at `e_phoff`/`e_phentsize` and refuses unless no `PT_INTERP` is present; the kernel reads the same three fields, so they agree. `PN_XNUM`, a table outside the bytes, overflowing `e_phoff`/`e_phoff*entsize`, a zero-size or last-position `PT_INTERP`, a 32-bit image, a truncated table, and a non-ELF/script all resolve to `Foreign`/`Unreadable`/`Interpreted` (refused). The `pt-interp` and `foreign` guards are mutation-covered.
- **The helper is first in the view on every listed start.** A list with a controlling terminal is refused `session.pty-unserved` in both places (`process.rs:1180` dispatch, `process.rs:2958` argv builder, inside the `ChildChannel::Terminal` arm so the `setsid --ctty` argv is never built); on a listed start no `--setenv` and no `/usr/bin/env` stage is emitted (`gate.is_none()` guards them). A daemon vector asserts `session.pty-unserved` (`runtime_vectors.rs:3742`).
- **The static helper runs nothing before the ruleset.** `/etc/ld.so.preload`, `LD_PRELOAD` and `ld.so.cache` have no loader to act on them (adversary2 passes); the helper opens paths O_PATH for the ruleset and reads bubblewrap's own `/proc` for placement — no file content is executed.

## What I could not run / did not run (judgement)

- Host-crate mutation of the two terminal refusals: not executed (2.1 GB target, disk tight). Both code paths exist; a daemon `session.pty-unserved` vector exists. Not independently mutated.
- `binfmt_misc`, vDSO, `PT_GNU_STACK`, and a client-supplied helper: out of the real threat model — the run cannot mount (no `CAP_SYS_ADMIN`, `--disable-userns`); vDSO/`GNU_STACK` are kernel-provided, not profile-controlled; and a client cannot set `exec_gate`, so the ELF test defends against operator misconfiguration only. Reasoned, not run.
- `systemd` around `linux` and the probe sandbox: the systemd adapter admits on the inner `linux` adapter through the same `command()` builder, and the probe builds its own argv. Read, not separately run.
- No attack area was skipped because of the earlier safety-classifier stop.

## Paths written outside the worktree

All under `<scratch>/adversary1/`: `lane-static.sh`, `mutate3-gate.sh`, and logs `f8-static.log`, `shadow.log`, `shadow2.log`, `adv-all.log`, `mutate3-gate.log`, plus the rebuilt `substrate-exec-gate.static` / `.dynamic`. Mutation target directories were removed (133 GB free at close).

**Net of this pass:** F8 is fixed by the static helper and my regression case proves it; the helper mount is not shadowable (fail-closed). No new S1/S2. Three S3s: two untested ELF guards (`PN_XNUM`, `entry_size`) and the un-reserved gate mount that refuses at the helper rather than before dispatch.
