---
format: aep.planning-md/3
id: review-result:wave-2026-10-23-b10b11b12-security-pass-1
kind: review-result
status: active
title: Security review pass 1, unit b10/b11/b12 (retire, nested cgroups, read-only roots admission)
relations:
- reviews: story:session-retire-waits-for-proven-absence
- reviews: story:nested-exec-cgroups-are-removed-depth-first
- reviews: story:host-admit-checks-the-read-only-roots-fact
revision: 1
---
# Security review pass 1, unit b10/b11/b12 (session retire, nested cgroups, read-only roots admission)

Agent type `aep:security-reviewer`. Branch `unit/2026-10-23-b10b11b12-retire-cgroups-admit`, commit `407270f`. Cases in `crates/substrate-host/tests/b10b11b12_security_pass1.rs`. Verdict: 1 CONFIRMED (b12, warning); b10 and b11 held.

```findings
- file: crates/substrate-host/src/read_only_roots.rs
  line: 83
  category: acceptance
  severity: warning
  verdict: CONFIRMED
  origin: introduced
  message: admit opens every declared root before checking the exec.read-only-roots fact, so on a host without the fact the driver resolves caller-named host paths and answers exec.read-only-root-absent or not-a-directory instead of the daemon's exec.sandbox-unavailable.
```

Red output: `ex_b12_absent: expected exec.sandbox-unavailable at read_only_roots, got exec.read-only-root-absent`; `ex_b12_file: … got exec.read-only-root-not-a-directory`.

Green: `delegated_b11_nested_groups_exactly_at_both_bounds_are_removed` (32 deep and 256 groups removed) and `delegated_b11_one_group_past_the_count_bound_keeps_the_run_unknown` (257 groups keep the run Unknown).

Held: b10 retire check inside one immediate transaction, physically_absent reset by upsert_exec, no other path to Retired; b11 walk cannot leave the exec cgroup (no /sys in the sandbox, cgroupfs has no symlinks, a concurrent child or process makes rmdir fail and the tree is kept); the changed 33-deep tests still prove the unknown path.

Coordinator decision: the driver follows the daemon's order (lexical, fact, then open); the two portable per-root conformance cases move to the delegated lane, where the fact is published.
