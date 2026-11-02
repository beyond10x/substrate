---
format: aep.planning-md/3
id: review-result:wave-2026-11-02-d364-u3-security-pass-1
kind: review-result
status: active
title: 'D364 U3 security pass 1: the forwarder pidfd and its descriptors'
relations:
- reviews: story:aperture-install-sees-forwarder-death-at-once
revision: 1
---
# Security pass 1 — U3 of wave D364

Unit `story:aperture-install-sees-forwarder-death-at-once`, diff `8b9818dc...48b82224` (the fix
`cd572120` plus the adversary's cases), merged into `wave/2026-11-02-d364` at `87092f45`. Report as
returned by the security reviewer (`aep:security-reviewer`, Opus); machine paths shortened to the
unit's scratch. The reviewer was stopped by the pre-check before fmt; the coordinator ran fmt (one
assertion rewrapped), clippy and the host lib suite and committed the case as `b896b419`.

```
unit: story:aperture-install-sees-forwarder-death-at-once
verdict: blocked (fmt refused by the pre-check: / 79G); coordinator finished: nothing found
cases: executed 42→42 passed (egress filter, includes the new d364_sec case), red 0
origin: introduced 0, pre-existing 2, undecided 0
wrote-outside-worktree: one log in the unit's security scratch
needs-coordinator: yes (fmt, clippy, the host lib run, the commit; done)
```

Case in `crates/substrate-host/src/egress.rs`, `mod d364_sec`:
`the_report_wait_descriptor_names_the_own_child_and_leaves_with_the_refusal`, own-process helper
`window_descriptors_helper` (marked `#[ignore = "child process entry point: run by …"]`), reached
its marker `D364-SEC window-descriptors`. Egress lane 42 passed, 7 ignored; after the
coordinator's fmt: clippy `--all-targets -D warnings` exit 0, `-p b10x-substrate-host --lib
--locked` 421 passed, 16 ignored.

Invariants:

1. The pidfd names the installer's own unreaped child: measured inside the window (one pidfd whose
   fdinfo shows the forwarder's pid; `parent_of(forwarder) == getpid()`). Nothing in the daemon
   reaps early: `waitpid(-1)` is only in substrate-executor, a separate binary; `SIGCHLD SIG_IGN` is
   set only inside the forwarder after the fork (`egress.rs:1029`); this part is code-only inference.
2. Every descriptor is close-on-exec and none leaks: `O_CLOEXEC` in the fdinfo flags of the pidfd
   and the report end; after the stage-6 refusal the forwarder is reaped, no child left, pidfd set
   and descriptor table unchanged. The adversary's cases cover success and EMFILE.
3. A pidfd that cannot be opened is refused (`egress.rs:886-895`): neither waits nor admits.
4. The test hook exists only under `#[cfg(test)]` (`:787`, `:790`, `:896`); no feature exposes it.
5. The daemon seccomp profile admits `pidfd_open` (`:206`), `poll` (`:213`), `ppoll` (`:214`) in its
   unconditional allow block, selected at `deploy/helm/substrate/templates/statefulset.yaml:254-256`.

```findings
[{"file": "crates/substrate-host/src/egress.rs", "line": 886, "category": "boundary", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "scope in; case the_report_wait_descriptor_names_the_own_child_and_leaves_with_the_refusal: inside the report window exactly one pidfd names the forwarder, which is the installer's own unreaped child; that pidfd and the report end are O_CLOEXEC, and after the stage-6 refusal the forwarder is reaped with the pidfd set and descriptor table unchanged."},
 {"file": "crates/substrate-host/src/egress.rs", "line": 966, "category": "judgement", "severity": "note", "verdict": "INFEASIBLE", "origin": "pre-existing", "message": "scope out; case none: a 1-byte record from any holder of the report socket's other end is admitted without checking the sender (no SO_PEERCRED/SCM_CREDENTIALS); the only other holders are threads' forks in the window, which close it at exec (SOCK_CLOEXEC) and write nothing, so there is no production path."},
 {"file": "crates/substrate-host/src/egress.rs", "line": 919, "category": "judgement", "severity": "note", "verdict": "CONFIRMED", "origin": "pre-existing", "message": "scope out; case none: abandon_forwarder's waitpid does not retry on EINTR, so a handler installed without SA_RESTART could leave a refused forwarder as a zombie; the code was moved here, not changed."},
 {"file": "crates/substrate-host/src/egress.rs", "line": 889, "category": "judgement", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "scope in; case none: a runtime whose seccomp denies pidfd_open (kernel older than 5.3, or a profile without it) now refuses every aperture install as 'aperture forwarder descriptor' instead of falling back; this fits invariant 3, and deploy/seccomp/host-exec-v1-amd64.json:206 admits pidfd_open."},
 {"file": "deploy/seccomp/host-exec-v1-amd64.json", "line": 206, "category": "contract-drift", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "scope in; case none: pidfd_open (:206), poll (:213) and ppoll (:214) are in the unconditional SCMP_ACT_ALLOW block (lines 10-373) of the profile that deploy/helm/substrate/templates/statefulset.yaml:254-256 selects."},
 {"file": "crates/substrate-host/src/egress.rs", "line": 896, "category": "boundary", "severity": "note", "verdict": "CONFIRMED", "origin": "introduced", "message": "scope in; case none: IN_REPORT_WINDOW, its type and its call site exist only under #[cfg(test)] (:787, :790, :896) and no feature enables them, so the hook cannot exist in a release build."}]
```
