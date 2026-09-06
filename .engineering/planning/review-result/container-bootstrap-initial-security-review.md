---
format: aep.planning-md/1
id: review-result:container-bootstrap-initial-security-review
kind: review-result
status: active
title: Initial independent container bootstrap security review
relations:
- reviews: story:container-private-exec-bootstrap
revision: 1
---
needs-revision

Independent read-only review of `docs/design/25-container-private-host-execution.md`, `crates/substrate-daemon/src/bin/substrate-container-exec.rs`, and `crates/substrate-daemon/src/container_bootstrap.rs` over base `1ceb11bbaa38221ecf3e48a62d41271d9ae57d0f`. Two startup-admission defects need correction. These are conditional states accepted by the code, not findings that the deployed image or node currently has either unsafe state.

1. **Validate the ownership and permissions that protect enclosing limits.** `crates/substrate-daemon/src/container_bootstrap.rs:105` reads the enclosing ceilings, and the comparisons at lines 33 and 35 establish that their values have not changed during bootstrap. Neither these checks nor `make_delegation` validates the ownership or write permissions of `cpu.max`, `memory.max`, `memory.swap.max`, or `pids.max`. A finite delegation whose limit files are already writable by UID/GID 65532 passes these checks. After the remount and UID drop, the daemon retains that access wherever an independent kernel/LSM restriction does not deny it. Thus the code does not establish the design's stated guarantee that the non-root daemon cannot raise its enclosing ceilings. This is an admission gap rather than a demonstrated problem in the current runtime. Require the enclosing control files to be root-owned with no group/other write before mutation, reject unsafe initial metadata, and verify the resulting unprivileged process cannot obtain write access without writing or truncating those files. Add negative coverage for a delegate-owned or group/other-writable ceiling file. The kernel explicitly distinguishes delegation of structural files from granting write access to the parent's resource controls. [Kernel cgroup v2 delegation documentation](https://www.kernel.org/doc/html/latest/admin-guide/cgroup-v2.html#delegation)

2. **Reject setuid/setgid selected executables before the privilege drop.** `crates/substrate-daemon/src/container_bootstrap.rs:26` accepts any root-owned regular selected executable without group/other write; mode `04755` passes. In quota mode, preflight requires `no_new_privs` to be clear and the final drop intentionally leaves it clear so the existing file capability can be acquired. Executing a setuid-root quota binary can therefore restore effective/saved root identity after the UID verification at line 250, even with the reduced capability bounding set. This contradicts the fixed non-root daemon transition and can also undo the intended DAC protection of root-owned cgroup controls. Reject both setuid and setgid bits for either selected executable, and add metadata-predicate regressions for those modes while retaining the intended ordinary executable and quota file-capability case. This does not assert that the current image contains a setuid daemon. [Linux capabilities documentation on setuid execution and file capabilities](https://man7.org/linux/man-pages/man7/capabilities.7.html)

The remaining inspected sequence is consistent with the bounded design. The entrypoint accepts ordinary daemon arguments rather than a caller-selected program and rejects explicit cgroup-root overrides before appending its fixed root. PID 1, root identity, exact cgroup namespace-relative membership, a single cgroup2 mount rooted at the namespace root, required mount flags, no propagation metadata, direct membership, no existing child groups, and required controllers are checked before mutation. The combined membership and mount-root checks reject an ancestor hierarchy view; this relies on the runtime supplying authentic procfs/cgroupfs views and the promised private container namespace setup. It is not a proof against an operator supplying forged mounts or a different executable image. [Linux cgroup namespace documentation](https://man7.org/linux/man-pages/man7/cgroup_namespaces.7.html)

The bind-remount uses `MS_BIND | MS_REMOUNT`, preserves the four specifically required mount flags, and changes only the selected mount's flags rather than remounting the cgroup filesystem superblock. The process moves into the fixed child before controller enablement and verifies an empty direct-member list; kernel controller enablement remains an additional refusal point for an invalid populated topology. Ownership changes enumerate structural delegation paths rather than recursively changing the ceiling files. [Linux mount documentation](https://man7.org/linux/man-pages/man2/mount.2.html)

The privilege transition checks an exact starting capability set and zero securebits/inheritable/ambient sets, removes unwanted bounding capabilities, clears supplementary groups, replaces GID/UID, and verifies all four UID/GID identities and capability sets. The ordinary branch then sets `no_new_privs`; the quota branch retains only the intentional SYS_ADMIN bounding allowance. The fixed `exec` happens after these checks; finding 2 concerns the executable metadata needed to preserve their result across that final transition. Errors return before this code starts a daemon listener. Incomplete bootstrap cleanup remains owned by container teardown as documented.

Retained `bootstrap-tests.log` records two passing pure validation tests; `bootstrap-clippy-final.log` records successful targeted Clippy completion. Those tests cover selected mount/refusal parsing and finite CPU/cgroup argument checks. They do not exercise the privileged remount, actual UID/capability transition, limit-file write denial, or post-exec daemon identity. This review ran no builds, privileged probes, or service mutations. The design's real-container ordinary/quota, parent-limit denial, worker confinement, PTY and cleanup evidence remains required before publication. The AppArmor profile, Dockerfile, node installation, and end-to-end terminal acceptance are outside this bounded verdict because those inputs are still being revised. No live workspace or session was touched.

Reviewed file SHA-256 identities:

```text
fee8e8c5b4584cccabce049eed83a06335cc83ae15682504323282a8a8f17ba4  docs/design/25-container-private-host-execution.md
10f656ee4512de004d6b3b67babacc400cf53e9f30a7573d8edf76ecfd4286a1  crates/substrate-daemon/src/bin/substrate-container-exec.rs
aed526454e51ebd26472c6836f92cfe46658c9809bc21aae7591dc45e0ae6806  crates/substrate-daemon/src/container_bootstrap.rs
```

```findings
[
  {
    "file": "crates/substrate-daemon/src/container_bootstrap.rs",
    "line": 105,
    "category": "security",
    "severity": "blocker",
    "message": "Enclosing-limit checks compare values without validating ownership/write permissions. An initially delegate-writable ceiling file is accepted, so the non-root daemon's inability to raise its enclosing limits is not established. Refuse unsafe root control-file metadata and verify write-access denial after the UID drop, with negative coverage."
  },
  {
    "file": "crates/substrate-daemon/src/container_bootstrap.rs",
    "line": 26,
    "category": "security",
    "severity": "blocker",
    "message": "The fixed executable predicate accepts setuid/setgid files. Quota mode deliberately leaves no_new_privs clear, so a setuid-root selected executable can restore root identity after the final UID checks. Reject both special mode bits and regression-test those refusals."
  }
]
```
