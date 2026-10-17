---
format: aep.planning-md/3
id: review-result:security-hostproc-u4-pass-1
kind: review-result
status: active
title: Security review pass 1, unit u4 (host-process driver)
relations:
- reviews: story:host-process-cgroup-tree
revision: 1
---
# Security review pass 1, unit u4 (host-process driver)

Agent type `aep:security-reviewer`. Branch `unit/2026-10-17-u4-hostproc-host`, head `088f284e`. Cases in `crates/substrate-host/src/host_process/security_tests.rs`. Verdict: NEEDS-CHANGE; fixed in `9f53478d`.

Tests: crates/substrate-host/src/host_process/security_tests.rs (untracked; add `#[cfg(test)] mod security_tests;` after `mod tests;` in mod.rs)
1 blocker mod.rs:768/521/1065: main process leaving its cgroup (exec systemd-run --scope) -> stop/lease/deadline find empty cgroup and wait forever on child.wait. Fix: after terminate, if main unreaped, start_kill() it (channel to supervise). Case delegated_stop_ends_a_main_process_that_left_its_cgroup_and_returns.
2 warning mod.rs:1078-1088, process.rs:3450: member-created child cgroup -> rmdir EBUSY -> process.cgroup-failed + leak. Fix: remove descendants depth-first after populated 0. Case delegated_a_nested_cgroup_a_member_creates_does_not_keep_a_clean_exit_from_ending.
3 warning mod.rs:522, process.rs:3419: SIGTERM only to top-level cgroup.procs. Fix: walk subtree. Case delegated_stop_sends_sigterm_to_members_of_nested_cgroups_too.
4 warning process.rs:3419: pid recycle between read and kill. Fix: cgroup.freeze, read, signal, thaw (or pidfd).
5 note mod.rs:257: derived Debug on HostProcessSpec prints env values. Fix: manual Debug redacting values. Case a_spec_debug_form_carries_no_environment_value.
6 note ADR/design: same-uid process can leave its cgroup; S3 "dies with its record" holds only for members that do not move themselves — document.
7 note ports.rs:116: /proc reads of recycled pids, unbounded fd/socket tables — bound them.

Fixes: 1 a pidfd on the main process, opened while it is the unreaped child, ends it on stop, lease expiry and deadline even when it left its cgroup; 2 the subtree is removed children first; 3 SIGTERM walks the subtree; 4 each member is pinned by pidfd and re-checked against cgroup.procs before signalling; 5 Debug redacts environment values; 7 port observation is bounded (4096 members, 65536 fds, 16 MiB per table) and absent past a bound; 6 recorded in ADR 0036 consequences.
