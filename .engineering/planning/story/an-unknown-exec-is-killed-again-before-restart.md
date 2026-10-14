---
format: aep.planning-md/3
id: story:an-unknown-exec-is-killed-again-before-restart
kind: story
status: draft
title: An exec whose tree was not proven empty is killed again before the daemon restarts
relations:
- informed_by: story:session-containment-retries-an-unproven-kill
revision: 1
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
