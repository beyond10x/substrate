---
format: aep.planning-md/3
id: review-result:wave-2026-10-14-parallel-safety-round-2
kind: review-result
status: active
title: parallel-safety critic (aep:plan-critic-parallel-safety), wave 2026-10-14 round 2
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
approve

Round 1's `CHANGELOG.md` finding is resolved, and the revisions add no new collision across the three units.

- **CHANGELOG.md (round 1):** Resolved. The retry story, the directory-listing story and the tracking story each now say the integrator writes the entry at integration, not the unit. They cite `.engineering/planning/story/session-containment-retries-an-unproven-kill.md:73`, `.engineering/planning/story/directory-listing-survives-special-entries.md:73-75` and `.engineering/planning/story/the-daemons-detached-containment-task-is-tracked.md`. The two entries are no longer written by concurrent units. The cgroup story says it owes none (`.engineering/planning/story/runtime-vector-cgroup-teardown-is-flaky.md:54`).
- **`service.rs` `sweep_expired`:** The retry story's scope now lists `crates/substrate-daemon/src/app/service.rs`. No other story in the set names it. Its other readers are `crates/substrate-daemon/tests/contract_vectors.rs` and `crates/substrate-daemon/tests/pipe_session.rs`, which call `sweep_expired` and do not edit it. `nudge_maintenance` is also called from `app/workspaces.rs` (lines 1103 and 1152), as a caller only. Nothing in the set edits that file, so the claim that no other unit touches `service.rs` holds.
- **Source-scanning tests:** The `app/metrics.rs` and `app/tests.rs` tests that read `sessions.rs` match `.on_upgrade(`, `.with_upgrades()` and the raw `upgrade::on(`. Removing the `tokio::spawn` in `on_failed_upgrade` does not trip them, so unit 1 needs no edit to `metrics.rs`. The `terminate_pipe_session` call at `metrics.rs:1498` is a string fixture, not a real caller.
- **Unit 2:** It is confined to `crates/substrate-daemon/tests/runtime_vectors.rs`, plus `scripts/delegated-lane.sh` only if the race is there. Nothing else in the set touches either. Under outcome 2 it files the daemon half and does not edit `sessions.rs` or `runtime.rs`.
- **Unit 3:** It lands on `crates/substrate-host/src/fs.rs` and `website/docs/reference/contract.md`. No other story names either file.

**What I read:** 4 stories, plus `review-result:wave-2026-10-14-parallel-safety-round-1`. I ran `aep plan artifact show` on the review result, `aep plan artifact graph` and `aep plan artifact waves --status draft`. I also used `git grep` and `sed` over `crates/substrate-daemon/src/app/`, `crates/substrate-daemon/tests/`, `crates/substrate-host/src/fs.rs` and `CHANGELOG.md`.

**Surfaces:** 4 cited, 0 inferred, 0 unplaceable. I traced 1 acceptance read to a producer in the set: the tracking story reads the pending-containment set the retry story adds, and 1 `depends_on` edge records it (tracking depends on retry).

**What I could not establish:**
- The directory story's "a test creates those three entries and reads both routes" does not name a test file. It could be the `fs.rs` tests module or `crates/substrate-daemon/tests/http.rs`, which already reads `/v2/workspaces/{workspace}/tree`. Neither file is claimed by another story in the set, so the verdict does not change.
- `aep plan artifact waves` still reports collisions between these stories and stories outside the set, on `sessions.rs` and `runtime.rs` with `story:transport-admission-and-stream-lifetime-disagree`. Those are outside what I was asked to judge.
- Whether the retry and tracking stories are the right split is the design critic's lane, and whether their acceptances are checkable is the acceptance critic's. Neither affects my verdict.

```findings
[]
```
