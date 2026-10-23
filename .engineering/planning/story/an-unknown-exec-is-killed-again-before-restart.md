---
format: aep.planning-md/3
id: story:an-unknown-exec-is-killed-again-before-restart
kind: story
status: implemented
title: An exec whose tree was not proven empty is killed again before the daemon restarts
relations:
- informed_by: story:session-containment-retries-an-unproven-kill
scope:
- confidence: cited
  path: crates/substrate-daemon/src/app/service.rs
- confidence: inferred
  path: crates/substrate-daemon/src/runtime.rs
- confidence: inferred
  path: crates/substrate-host/src/lib.rs
- confidence: cited
  path: crates/substrate-host/src/process.rs
revision: 8
transitions:
- {from: "draft", to: "proposed", at: "2026-10-23T06:27:15Z", actor: "agent:b10x-chef-orchestrator", revision: 6}
- {from: "proposed", to: "active", at: "2026-10-23T06:27:15Z", actor: "agent:b10x-chef-orchestrator", revision: 7}
- {from: "active", to: "implemented", at: "2026-10-23T14:58:54Z", actor: "agent:b10x-chef-orchestrator", revision: 8, decided_on: {"recorded":{"test_result":1,"review_outcome":3,"verification":2}}}
---
# Story: An exec whose tree was not proven empty is killed again before the daemon restarts

## Context

The host driver records an exec `unknown` when `reconcile_cgroup` cannot prove its cgroup empty
after ten `kill_all` attempts (`crates/substrate-host/src/process.rs`, `reconcile_cgroup`). After
that, `signal` returns the cached observation without killing anything, because `is_terminal`
includes `Unknown`. Lease cleanup (`crates/substrate-daemon/src/app/service.rs`,
`cleanup_expired`) does select `Unknown` execs and sends `Kill`, but that call reaches the cached
answer, so nothing retries the tree until `reconcile_orphans` runs at the next daemon start.

Found by the second security pass of the session-containment unit (wave 2026-10-14): the
containment step drops an `unknown` exec with a warning, which keeps invariant 3, but no later step
re-kills its tree.

## Acceptance

A case whose driver's cgroup still holds a process after the exec was recorded `unknown` shows that
process killed by lease cleanup (or by a named step) within one maintenance round after the lease
expires, without a daemon restart; and a tree that still cannot be proven empty stays reported as
`unknown`, never as terminated.

## Notes

This is a behaviour change in the host driver and touches the whole-tree kill of the safety
envelope; it needs the design note named by invariant 8 if it changes an observation.

## Scope

Derived 2026-10-23 by `story-scoper`. Each line is **cited** (read from the story or the tree) or
**inferred** (a reading that could be wrong).

- **Primary surface:** `crates/substrate-host` (the cgroup kill in the host driver) — cited
- **Files:** `crates/substrate-host/src/process.rs` (`reconcile_cgroup` :3149, `signal` :1638, `is_terminal` :3393, the `Unknown` assignment :2754, `acknowledge` :1556, `reconcile_orphans` :459) — cited
- **Files:** `crates/substrate-daemon/src/app/service.rs` (`cleanup_expired` :720; `sweep_expired` :392 calls `persist_completed_execs` :411 before lease cleanup) — cited
- **Also likely:** `crates/substrate-host/src/lib.rs` (`trait Driver` :539) if the retry becomes a driver step with a default method; `crates/substrate-daemon/src/runtime.rs` if it gets its own maintenance tick; `CHANGELOG.md` — inferred
- **Tests:** an in-crate test beside `contain_spawned_with_reconciliation` in `process.rs`; the delegated lane if a real cgroup is needed — inferred
- **Confidence:** medium — where the retry lives is open
- **Would collide with:** any unit on the exec lifecycle in `substrate-host/src/process.rs` or lease cleanup in `substrate-daemon/src/app/service.rs`
- **Mechanism correction (unproven, level 2–3):** `persist_completed_execs` (`service.rs:708`) triggers `acknowledge` (`process.rs:1556`), which removes the entry before lease cleanup runs, so `signal` usually returns `NotFound` (not a cached answer) and `cleanup_expired` closes the lease without killing. The cgroup directory outlives the entry (`Cgroup` has no `Drop`, `process.rs:3525`), so a retry can reopen it with `Cgroup::existing`. `Unknown` at `:2754` also comes from capsule, scratch and metrics failures, so the retry must remember that the cgroup itself was unproven.
