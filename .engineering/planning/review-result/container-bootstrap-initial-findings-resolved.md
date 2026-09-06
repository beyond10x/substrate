---
format: aep.planning-md/1
id: review-result:container-bootstrap-initial-findings-resolved
kind: review-result
status: active
title: Bootstrap ownership and executable admission findings resolved
relations:
- reviews: story:container-private-exec-bootstrap
revision: 1
---
approve

Independent read-only re-review of the three bootstrap/design files over base `1ceb11bbaa38221ecf3e48a62d41271d9ae57d0f`. Both findings in `bootstrap-independent-review.md` are resolved in the reviewed revision. No remaining actionable findings within this bounded source review. The initial report remains unchanged.

The enclosing-limit protection now uses `symlink_metadata` and the shared `safe_root_file` predicate on every `limits()` read (`crates/substrate-daemon/src/container_bootstrap.rs:150`). The predicate requires a regular file owned by root and rejects group/other write and setuid/setgid bits (`crates/substrate-daemon/src/container_bootstrap.rs:168`). This applies before the remount, after delegation, and after the UID drop. In addition, each of the four enclosing ceiling files must refuse a write-only open with `PermissionDenied` after all UID/GID and capability checks have completed (`crates/substrate-daemon/src/container_bootstrap.rs:35`). These opens neither create nor truncate files and perform no write. An unexpected successful open or any other error refuses startup before daemon exec. The final value comparison remains intact.

The fixed ordinary/quota executable is checked with the same predicate before any bootstrap mutation (`crates/substrate-daemon/src/container_bootstrap.rs:26`). Setuid and setgid modes are therefore rejected even in quota mode, where privilege acquisition remains intentionally enabled for the existing SYS_ADMIN file capability. The ordinary/quota selection, fixed appended cgroup root, bounding-set reduction, supplementary-group clearing, UID/GID transition, and ordinary `no_new_privs` setting remain unchanged. No caller-selected executable or new authority path was added.

The added regression (`crates/substrate-daemon/src/container_bootstrap.rs:317`) accepts ordinary root-owned regular-file modes and rejects delegate ownership, group/other write, individual and combined setuid/setgid bits, symlinks, and directories. Both production call sites use this tested predicate. Retained `bootstrap-tests-revision.log` records all three targeted tests passing with no ignored cases, and `bootstrap-clippy-revision.log` records successful targeted Clippy completion. These are coordinator-run results inspected by this reviewer; no build or test was rerun here.

The namespace validation, fixed bind-remount, process-free delegation sequence, explicit structural ownership changes, and privilege-drop checks retain their earlier reviewed behavior. Their security assumptions still include authentic procfs/cgroupfs views and the runtime's promised private namespace setup. The revised checks resolve the two unsafe startup states identified in the initial review without broadening delegation or capability authority.

This approval covers the inspected bootstrap source and design only. Pure metadata tests do not establish real-container mount behavior, post-exec daemon/worker identities, parent-limit denial in the actual image, ordinary/quota compatibility, PTY operation, confinement, or cleanup. Those runtime checks and the complete repository gate remain separate required evidence. Dockerfile, AppArmor policy, node installation, release/deployment inputs, and end-to-end terminal acceptance were not assessed in this re-review and remain outside this verdict. No live service, workspace, or session was touched.

Reviewed file SHA-256 identities:

```text
fee8e8c5b4584cccabce049eed83a06335cc83ae15682504323282a8a8f17ba4  docs/design/25-container-private-host-execution.md
10f656ee4512de004d6b3b67babacc400cf53e9f30a7573d8edf76ecfd4286a1  crates/substrate-daemon/src/bin/substrate-container-exec.rs
24203c8d7e1ea3c6e36bade1b70951d29e679f131ebcfb94c7866552a9affd4f  crates/substrate-daemon/src/container_bootstrap.rs
```

```findings
[]
```
