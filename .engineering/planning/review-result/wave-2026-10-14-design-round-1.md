---
format: aep.planning-md/3
id: review-result:wave-2026-10-14-design-round-1
kind: review-result
status: active
title: design critic (aep:plan-critic-design), wave 2026-10-14 round 1
tags:
- critic
- review
relations:
- reviews: story:session-containment-retries-an-unproven-kill
- reviews: story:the-daemons-detached-containment-task-is-tracked
- reviews: story:runtime-vector-cgroup-teardown-is-flaky
- reviews: story:directory-listing-survives-special-entries
- reviews: story:daemon-image-serves-exec-or-says-it-cannot
revision: 1
---
needs-revision
story:the-daemons-detached-containment-task-is-tracked — its own body says it is "worked after" story:session-containment-retries-an-unproven-kill because "the retry changes the task this story gives an owner", but no `depends_on` edge records that, so `aep plan artifact waves` puts the tracking story in wave 2 and the retry story in wave 3, the reverse of the stated order; add `depends_on` from the tracking story to the retry story. — .engineering/planning/story/the-daemons-detached-containment-task-is-tracked.md:48, aep plan artifact waves
story:directory-listing-survives-special-entries — the no-wire-change decision is sound against invariant 6, because no released bundle byte changes and `truncated` is a bare boolean in `workspace-tree-read-v2.json`. It is weak against invariant 3. The v1 page silently omits entries, with only a prose note beside the routes and no machine-readable signal. On v2 it overloads `truncated`, which today means only that `limit_items` was reached (`fs.rs:806`), so a client that retries with a higher limit never gets a complete tree. The Decision section should record both as accepted trade-offs against invariant 3, or pick another signal. — .engineering/planning/story/directory-listing-survives-special-entries.md:37-39, crates/substrate-host/src/fs.rs:806

What I read: five artifacts (`aep plan artifact show` on each), plus `relations`, `graph` (280 edges in the whole store, including edges outside the set), `waves` and `validate` (valid). I also read `sessions.rs` (the spawn at 1341, `terminate_pipe_session` at 1925), `runtime.rs` (the three connection `JoinSet`s at 673, 1015 and 1099), the wire `DirectoryPage` and `WorkspaceTree` types, `contracts/substrate-wire/0.18.0/schemas/results/workspace-tree-read-v2.json`, and `fs.rs:764-840`.

Walk and traces:
- There is no cycle. The five stories have no `depends_on` edges among them, and I found none reaching them from outside.
- Acceptance reads traced to a producer in the set: 1, the tracking story's shutdown case reading the containment task the retry story reshapes. 0 of those are recorded by an edge.
- The other four units are independent: no cycle, no chain, no shared abstraction.
- The pair is a legitimate shared-file unit, and the only reason the edge is missing is that the order lives in prose. The alternative to the edge is to merge the two stories into one.

What I could not establish:
- Out of my lane (acceptance): the tracking story's acceptance requires "the same `JoinSet`" as the connection tasks. There are three such sets, one per transport (`runtime.rs:673`, `:1015`, `:1099`), none reachable from `app/`. The Notes then offer a maintenance-owned set, which contradicts the acceptance.
- Out of my lane (scope): `terminate_pipe_session` has a second caller in the `on_upgrade` future (`sessions.rs:1378`), which no story gives an owner.
- Out of my lane (scope): the retry story does not say whether the retry is a loop in the task or a `sweep_expired` backstop. Where it lands decides what the tracking story has left to own, so the edge finding above depends on that choice.
- Out of my lane (parallel-safety): `aep plan artifact waves` reports collisions between the session unit and several stories outside this set (`unattached-claimed-session-is-contained`, `transport-admission-and-stream-lifetime-disagree` and others) on `sessions.rs`, `service.rs` and `runtime.rs`, contrary to the stories' "no other unit of this wave touches" lines. The same applies to the README collision for the image story.
- The cgroup-teardown story's daemon half, if the investigation finds one, is filed separately, so I treat it as outside the set.

```findings
- file: .engineering/planning/story/the-daemons-detached-containment-task-is-tracked.md
  line: 48
  category: design
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "its own body says it is worked after story:session-containment-retries-an-unproven-kill because the retry changes the task this story gives an owner, but no depends_on edge records that, so aep plan artifact waves places it in wave 2 and the retry story in wave 3, the reverse of the stated order; add depends_on from this story to the retry story"
- file: .engineering/planning/story/directory-listing-survives-special-entries.md
  line: 39
  category: design
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the no-wire-change decision is sound against invariant 6 but its Decision section does not record the invariant-3 cost: the v1 page omits entries with no machine-readable signal, and v2 reuses truncated, which today means only that limit_items was reached (fs.rs:806), so a client retrying with a higher limit never gets a complete tree; record both as accepted trade-offs or pick another signal"
```
