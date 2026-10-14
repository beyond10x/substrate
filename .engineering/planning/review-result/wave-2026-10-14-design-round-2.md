---
format: aep.planning-md/3
id: review-result:wave-2026-10-14-design-round-2
kind: review-result
status: active
title: design critic (aep:plan-critic-design), wave 2026-10-14 round 2
tags:
- critic
- review
relations:
- reviews: story:session-containment-retries-an-unproven-kill
- reviews: story:the-daemons-detached-containment-task-is-tracked
- reviews: story:runtime-vector-cgroup-teardown-is-flaky
- reviews: story:directory-listing-survives-special-entries
revision: 1
---
needs-revision

Round 1 is resolved. The `depends_on` edge is present, and the directory-listing Decision now records both invariant-3 trade-offs. The new Decision sections add three problems, listed below.

```
story:session-containment-retries-an-unproven-kill — the Decision runs `terminate_pipe_session` for every pending entry inside `App::sweep_expired`, which holds the `lease_sweep` mutex and runs on the daemon's single sweeper task; that call waits up to `MAINTENANCE_DRIVER_TIMEOUT` (30 s) per entry, so a driver that hangs on kill stalls reconcile, lease cleanup and every other maintenance duty for 30 s per pending entry, and the acceptance "within one maintenance tick" cannot hold under that failure; the Decision must say how the step is bounded (a cap on entries per tick, attempts run concurrently with a short timeout, or run outside the `lease_sweep` guard) — crates/substrate-daemon/src/app/service.rs:322-323, crates/substrate-daemon/src/app.rs:16, crates/substrate-daemon/src/runtime.rs:639-647, .engineering/planning/story/session-containment-retries-an-unproven-kill.md:29-33
story:session-containment-retries-an-unproven-kill — the pending set drops an entry only when the kill returns `true` or the exec is "already terminal", and the Decision names no attempt or age bound and no read that decides "terminal", so an entry for an exec the driver can no longer signal (or whose store row is gone) is retried every 250 ms until restart, each time costing a driver timeout under the sweep guard; add a drop rule, for example the exec's lease expiring (when `cleanup_expired` takes over) or a bounded attempt count that logs `tracing::warn!` when it gives up (invariant 3) — .engineering/planning/story/session-containment-retries-an-unproven-kill.md:29-33
story:the-daemons-detached-containment-task-is-tracked — the Decision moves the first containment attempt after a failed upgrade from an immediate detached task into `sweep_expired`, so the kill now waits behind whatever the in-flight sweep is doing (reconcile, lease cleanup and driver calls of up to 30 s each), which makes the unattached-process window depend on unrelated maintenance; the Decision does not record this cost, and the acceptance only proves the kill happens after a driven sweep, so state the latency trade-off or have the first attempt run immediately with only the retry in the sweep — .engineering/planning/story/the-daemons-detached-containment-task-is-tracked.md:25-30, crates/substrate-daemon/src/app/service.rs:322-323
```

What I read: 6 artifacts, plus `~/.local/state/worktree/trees/b10x/substrate/w15-plan`'s store diff. Commands run:
- `aep plan artifact show` on the four set stories, the new `story:directory-listing-names-omitted-entries` and `review-result:wave-2026-10-14-design-round-1`
- `aep plan artifact relations`, `graph`, `waves` and `validate` (valid)
- `service.rs:270-420`, `sessions.rs:1300-1420` and `:1900-1945`, `runtime.rs:625-670`, `app.rs:16`, `fs.rs:760-840`

The graph has 280 edges store-wide, and I walked the ones touching the set. There is no cycle and no serialising chain.

Round-1 findings:
- **depends_on finding: resolved.** `story:the-daemons-detached-containment-task-is-tracked` now `depends_on` the retry story, and `waves` puts them in order (wave 6 after wave 3).
- **Invariant-3 finding: resolved.** `story:directory-listing-survives-special-entries:36-47` records both trade-offs, and `story:directory-listing-names-omitted-entries` is a draft outside the set that carries the real signal. It sets no edge on the set. It correctly needs a successor bundle and an ADR.

Other results:
- The new follow-up and the cgroup-teardown story are independent of the rest.
- I traced one acceptance read to a producer in the set (the tracking story's sweep case reads the pending set the retry story adds), and the edge records it.

Could not establish, or outside my lane:
- **Shutdown semantics.** The tracking story's Context says the task is "neither awaited nor cancelled". Under the new Decision, `lease_sweeper.abort()` cancels an in-flight kill and drops queued entries, as before. The body does not say whether cancel-on-abort is the intended meaning of "tracked", and the acceptance does not test it. This is an unease rather than a finding, and it is partly an acceptance matter.
- **Out of my lane (parallel-safety).** `waves` still reports collisions between the session unit and stories outside the set on `sessions.rs`, `service.rs`, `runtime.rs` and `tests/pipe_session.rs`. They include `story:unattached-claimed-session-is-contained` and `story:lease-cleanup-reads-exec-state-only`. They contradict the "no other unit of this wave touches" lines, which are scoped to the wave and so are only literally true of it.
- **Out of my lane (acceptance).** `website/docs/reference/contract.md` has no `truncated` text today, so the directory story's documentation acceptance has no existing sentence to extend.
- **Invariant 5.** I judged the in-memory pending set compatible with it, since kill signals are not ledger operations and the restart path is stated. I did not verify what `reconcile_after_restart` does to a live exec.

```findings
- file: crates/substrate-daemon/src/app/service.rs
  line: 323
  category: design
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the Decision runs terminate_pipe_session for every pending entry inside App::sweep_expired, which holds the lease_sweep mutex on the single sweeper task; each call waits up to MAINTENANCE_DRIVER_TIMEOUT (30 s), so a driver that hangs on kill stalls reconcile, lease cleanup and all other maintenance for 30 s per pending entry and the acceptance 'within one maintenance tick' cannot hold; the Decision must say how the step is bounded (entry cap per tick, concurrent attempts with a short timeout, or outside the lease_sweep guard)"
- file: .engineering/planning/story/session-containment-retries-an-unproven-kill.md
  line: 33
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the pending set drops an entry only on a true kill or an already-terminal exec, with no attempt or age bound and no stated read that decides 'terminal', so an entry for an exec the driver can no longer signal is retried every tick until restart at a driver-timeout cost each time; add a drop rule (lease expiry handing over to cleanup_expired, or a bounded attempt count that warns when it gives up)"
- file: .engineering/planning/story/the-daemons-detached-containment-task-is-tracked.md
  line: 30
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the Decision moves the first containment attempt after a failed upgrade from an immediate detached task into sweep_expired, so the kill waits behind the in-flight sweep (reconcile, lease cleanup, driver calls of up to 30 s) and the unattached-process window depends on unrelated maintenance; record the latency trade-off or keep the first attempt immediate and put only the retry in the sweep"
```
