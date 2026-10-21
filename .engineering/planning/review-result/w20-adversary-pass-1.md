---
format: aep.planning-md/3
id: review-result:w20-adversary-pass-1
kind: review-result
status: active
title: Adversary pass 1, wave 2026-10-17b (host-process routes)
relations:
- reviews: story:host-process-start
- reviews: story:host-process-cgroup-tree
- reviews: story:host-process-deadlines
- reviews: story:host-process-lease
prose_only: findings carry severity and fix commit in prose, as the wave's other reviews do
revision: 1
---
# Adversary pass 1, wave 2026-10-17b (host-process routes)

Reviewed: 85ac6d9e..38dc0fd2 on `wave/2026-10-17-hostproc-routes` (PR 143). Verdict: needs change; F1, F2a-F2c fixed.

Findings:
- **F1, blocker, fixed in 38dc0fd2.** The host crate did not compile on macOS (E0425/E0433 in `host_process/mod.rs`); `check_cwd`, `REATTACHED_POLL` and `supervise_reattached` are now Linux-only, and off Linux `check_cwd` refuses as `process.host-unserved`.
- **F2a, should-fix, fixed in 69f390a8.** Restart reconciliation and `this_run` compared `started_at` with a wall-clock cutoff; a backward clock step across a restart orphaned a running process. Run membership is now a per-run id stored on each record.
- **F2b, should-fix, fixed in 69f390a8.** The same cause abandoned a dispatched start across a restart.
- **F2c, note, fixed in 69f390a8.** A step back inside a run left a dropped start unended.
- **F3, note, accepted.** Lease and deadline expiry read the wall clock; they measure time, not run membership.
- **F4, note, open.** `process.start-abandoned` (503) is named in no ADR or bundle.
- **F5, note, open.** The 0.20.0 host-process executable vectors are schema-checked only.

Same class outside host processes, open: `Store::reconcile_after_restart`, `recovery_workspaces` and `recovery_execs` still split runs by a wall-clock cutoff.

Found while verifying: `scripts/delegated-lane.sh` never ran the delegated cases of `--test processes`, and one of them expected the wrong refusal for a renew carrying `keep`. Fixed in aad58cc2; the lane ran green (`processes` 59 passed, exit 0).
