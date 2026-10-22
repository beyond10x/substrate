---
format: aep.planning-md/3
id: review-result:wave-2026-10-22-writable-roots-security-pass-1
kind: review-result
status: active
title: Security review pass 1, writable-roots wave (U2, U3)
relations:
- reviews: story:writable-host-roots
- reviews: story:writable-git-roots
revision: 1
---
Security review pass 1 on the writable-roots wave (U2 kind 1, U3 kind 2), against 04b59d80; fixes in ed0d9975. Cases in `crates/substrate-daemon/tests/writable_roots_security.rs` (3) and `crates/substrate-host/tests/writable_roots_security.rs` (2, delegated). Verdict: three findings, all fixed.

Checked, sound: lexical mount checks (reserved, duplicate, overlap) refuse before dispatch; the pinned descriptor carries O_CLOEXEC and leaks into no child; the host and Git facts stay absent until their probes pass; the owner check runs by fstat on the descriptor that is bound; operations are durable before dispatch; a declared host root is the only writable host mount in the sandbox.

- **S1, medium, fixed.** Lease expiry ended a workspace that another workspace's running exec bound as a Git root, without signalling that exec. Lease cleanup now signals every nonterminal exec holding the workspace (`Store::execs_holding_workspace`).
- **S2, low-medium, fixed.** An Accepted or Unknown exec did not hold its Git root, because in-use accounting read only the applied record. The bound workspace ids are recorded on the exec row at acceptance (`execs.git_root_workspaces`, with a migration).
- **S3, low, fixed.** Startup accepted a writable root that contains the API socket, the bubblewrap binary, a secret-slot source, the CA bundle or the delegated cgroup root. It now refuses each with `config.writable-root-invalid`, compared as written and as resolved.

Remaining, accepted: the protected-path check runs at startup only; per-start `openat2` pinning is unchanged. Kernel project quota on a bound checkout is not exercised (quota-lane fixture absent).

Full gate exit 0; delegated lane exit 0, 111 portable and 167 delegated runtime vectors.
