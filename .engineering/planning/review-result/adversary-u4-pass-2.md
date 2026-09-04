---
format: aep.planning-md/1
id: review-result:adversary-u4-pass-2
kind: review-result
status: active
title: Adversary pass 2, unit u4 (disable-userns)
relations:
- reviews: story:confined-processes-cannot-nest-user-namespaces
revision: 1
---
# Adversary pass 2 — unit u4, `story:confined-processes-cannot-nest-user-namespaces`

Agent type `adp:adversary`. Worktree `wt-5698af5e5dcf`, branch `impl/disable-userns`, HEAD `8ebfc74`,
base `617bbed`. Report as returned; the findings block is the agent's own, in the store's schema.

```
unit: u4 story:confined-processes-cannot-nest-user-namespaces
verdict: red
cases: executed 87→89, red 2
origin: introduced 7, pre-existing 1, undecided 0
wrote-outside-worktree: 9 paths
needs-coordinator: no
```

## Cases added

`crates/substrate-host/tests/user_namespace_floor_documents.rs`, 166 lines, untracked. No
implementation file touched. Both cases assert documentation states, both red.

```
running 2 tests
test the_deployment_list_names_the_backend_options_exec_now_requires ... FAILED
test every_document_the_floor_is_named_in_states_the_no_nested_user_namespace_clause ... FAILED

---- the_deployment_list_names_the_backend_options_exec_now_requires stdout ----
panicked at crates/substrate-host/tests/user_namespace_floor_documents.rs:152:5:
the list says only "provide the configured bubblewrap binary", which is no longer sufficient:
`probe_bubblewrap` puts `--disable-userns` and `--assert-userns-disabled` in its argv
(probe.rs:336, :381) and a backend that refuses either leaves every `exec` fact absent.

---- every_document_the_floor_is_named_in_states_the_no_nested_user_namespace_clause stdout ----
panicked at crates/substrate-host/tests/user_namespace_floor_documents.rs:119:5:
assertion `left == right` failed: design 15:147 names these documents as the floor, and the host now
withholds `exec.namespaces` outright when a backend cannot prove a non-nestable user namespace
  left: ["docs/design/04-security-and-isolation.md § 7, Minimum host guarantee"]
 right: []

test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out
```

Package gate with the file present: exit 101, 87 → 89 executed, lib 76 passed, all seven of the
unit's own targets green.

## What the adversary attacked and could not break

This is the load-bearing half of pass 2, because it is what says the security fix itself holds.

- **The acceptance statement is real, not a silent skip.** Straced under the delegated lane:
  `a_confined_process_cannot_nest_a_user_namespace` performs two real
  `execve("/usr/bin/bwrap", ["--unshare-user", "--disable-userns", …])` followed by
  `execve("/usr/bin/unshare", ["-U", "/bin/true"])`, and `host-pty-nested-userns` exists in the
  delegation root afterwards.
- **Nesting routes pass 1 did not try**, all inside the exact exec posture: nested
  `bwrap --unshare-user`, nested `bwrap` with implicit userns, `unshare -r`,
  `unshare --map-root-user`, `unshare -Um` — **all ENOSPC**. `unshare -m` alone EPERM. Writing
  `/proc/sys/user/max_user_namespaces` from inside: EACCES. Child capability sets all zero.
- **`--assert-userns-disabled` is a real observation, not a no-op** — fails with
  `creation of new user namespaces was not disabled as requested` when `--disable-userns` is absent,
  passes when present.
- **All eight bubblewrap sites** (by `Command::new` enumeration) splice the constant; the doc's
  count of eight is correct.
- **`--unshare-all` / `--unshare-user-try` / `--userns` and raw-string spellings** are caught by the
  scan; `["--unshare-user-try", "--disable-userns"]` would go red on `strays`.
- **design 15's row 1 citations** `process.rs:1905` and `probe.rs:381` are accurate at HEAD, its
  column 4 matches `sandbox_unavailable()` at `process.rs:2912-2917`, and `AGENTS.md:91-99` is the
  correct range.

## Findings, and the named fixes the adversary did not apply

1. add the clause to design 04 § 7's bullet list; 2. add the option requirement to README item 3;
3. add `concat!("\"--","userns2\"")` to `OTHER_SPELLINGS`, bump `[&str; 3]` → `[&str; 4]`, correct
"the three other spellings" at `:4380`; 4. either observe `egress.rs`'s test sandbox or say plainly
it is unobserved; 5. make the `--assert-userns-disabled` pin unconditional; 6. add
`assert!(expected > 0)` or delete the sentence; 7. correct "scratch" at `:890`.

`crates/substrate-host/src/seccomp.rs` was not attacked, per the brief. No finding there.

```findings
- file: docs/design/04-security-and-isolation.md
  line: 83
  category: contract-drift
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: >-
    design 15:147 declares design 04 section 7 to be part of the floor, but section 7's "requires all
    of the following before it advertises exec" list gained no no-nested-user-namespace clause while
    AGENTS.md:91-99 and design 15's table did; the code makes it a real precondition (probe.rs:381,
    :49), so the two floor documents now state different floors. Measured red at
    crates/substrate-host/tests/user_namespace_floor_documents.rs:119.
- file: README.md
  line: 271
  category: contract-drift
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: >-
    README section "Serving exec" item 3 still says only "provide the configured bubblewrap binary
    and /usr/bin/socat", but exec admission now requires a backend accepting --disable-userns and
    --assert-userns-disabled, so an operator on an older bubblewrap loses exec entirely with no
    documented cause. Measured red at
    crates/substrate-host/tests/user_namespace_floor_documents.rs:152.
- file: crates/substrate-host/src/process.rs
  line: 4396
  category: mutant
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: >-
    OTHER_SPELLINGS enumerates three other user-namespace spellings and omits --userns2 FD, which
    bwrap 0.11.2 lists and design 10:98-99 already records, and which bwrap accepts alongside
    --unshare-user --disable-userns (measured: "bwrap: Setting userns2 failed: Invalid argument",
    i.e. parsed and attempted, not rejected); the needle "--userns" does not match "--userns2", and
    --args FD defeats the scan entirely. Nothing in the crate reaches either today.
- file: crates/substrate-host/src/process.rs
  line: 4349
  category: contract-drift
  severity: note
  verdict: needs-revision
  origin: introduced
  message: >-
    The claim that "the source scan below is what covers" the eighth argv list is false. The scan
    asserts exactly one line carrying the literal "--unshare-user" and egress.rs:1467 carries the
    constant instead, so deleting that splice changes neither the site count nor the stray list. The
    scan can detect an added spelling, never a removed splice, at any site.
- file: crates/substrate-host/src/probe.rs
  line: 918
  category: mutant
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: >-
    Both guards over probe_bubblewrap's argv are conditional on /usr/bin/socat existing (the stub
    case returns at :918, the recorder pins 2 + usize::from(socat) at :986) and the source scan never
    looks for --assert-userns-disabled, so on a socat-less host deleting
    .arg("--assert-userns-disabled") at :381 leaves the whole package gate green. Security impact nil
    there because exec is unavailable anyway (probe.rs:320); what evaporates is the regression pin.
- file: crates/substrate-host/src/process.rs
  line: 104
  category: judgement
  severity: note
  verdict: needs-revision
  origin: introduced
  message: >-
    assert_recorded_posture's doc promises an "and unless one was recorded" empty check that the body
    does not contain — it is assert_eq!(len, expected), so assert_recorded_posture("x", &[], 0)
    passes on an empty log. Both current callers pass at least 2, so nothing reaches it today.
- file: crates/substrate-host/src/probe.rs
  line: 889
  category: contract-drift
  severity: note
  verdict: needs-revision
  origin: introduced
  message: >-
    The case doc lists "the capsule and scratch facts" among the facts gated on exec, but
    exec_scratch_quota is gated only on quota (probe.rs:135, :87), so with project_quota_ids
    configured a third exec.* fact survives a withheld floor where the source comment at :370-376
    says exactly two do. Not constructible on this host, so the reachable consequence is unmeasured.
- file: docs/design/10-destination-bound-egress.md
  line: 34
  category: contract-drift
  severity: note
  verdict: needs-revision
  origin: pre-existing
  message: >-
    The evidence row cites process.rs:901-905 for the sandbox argv, which lives at :1902-1928 and was
    already elsewhere at 617bbed (the row is byte-identical at base), and it still states the posture
    as "--unshare-net with --unshare-user/ipc/pid/uts", which is no longer the whole argv.
```
