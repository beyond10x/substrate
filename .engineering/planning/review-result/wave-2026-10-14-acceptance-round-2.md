---
format: aep.planning-md/3
id: review-result:wave-2026-10-14-acceptance-round-2
kind: review-result
status: active
title: acceptance critic (aep:plan-critic-acceptance), wave 2026-10-14 round 2
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

```
story:runtime-vector-cgroup-teardown-is-flaky — outcome 2 fires only on "a process still in the per-test group", but the Notes name a second cause ("or a child group still exists") and the Context records a second symptom (`500 operation.outcome-unknown` on exec 53), so an investigation that finds either fits neither outcome, and outcome 1 ("fix confined to the test harness") would admit a wait for the daemon's child group, which outcome 2 forbids — .engineering/planning/story/runtime-vector-cgroup-teardown-is-flaky.md:36
story:the-daemons-detached-containment-task-is-tracked — the acceptance's "Non-test … contains no `tokio::spawn`" has no stated check, and the Context's own command `grep -rn tokio::spawn crates/substrate-daemon/src/app/` also returns eight hits in `app/tests.rs` (a `#[cfg(test)]`-only file, `app.rs:28`) that the Context omits when it lists "`sessions.rs:1341`, plus four in `metrics.rs`", so two readers can get different answers; the body must name the exclusions (`app/tests.rs` and `metrics.rs` from line 415) — .engineering/planning/story/the-daemons-detached-containment-task-is-tracked.md:42
```

What I read: 4 of 4 ids given (story:session-containment-retries-an-unproven-kill, story:the-daemons-detached-containment-task-is-tracked, story:runtime-vector-cgroup-teardown-is-flaky, story:directory-listing-survives-special-entries), each in full with `aep plan artifact show`. I also read review-result:wave-2026-10-14-acceptance-round-1, `aep plan artifact lifecycle story` and `aep plan artifact relations`. In the tree I read `sessions.rs` (1341, 1378, 1925), `service.rs` (293, 322, 327, 643), `runtime.rs` (639–729, 1015, 1099), the pinned case at `tests/pipe_session.rs:2531`, `fs.rs:764-845`, `runtime_vectors.rs:60-100`, the tail of `scripts/delegated-lane.sh` and `xtask/src/quota_lane.rs`. I also grepped `tokio::spawn` across `app/`.

Round 1 status:
- **Resolved.** The JoinSet-versus-maintenance-owner blocker is closed: the acceptance now matches the Decision, naming the maintenance sweep as the one owner, with no JoinSet. The `runtime.rs` citations (639, 640, 651, 661, 673, 729, 1015, 1099) match the tree.
- **Resolved.** The directory-listing acceptance now names the documentation statement on both routes and the second meaning of `truncated`.
- **Partly resolved.** The cgroup story now has a second outcome, but it is too narrow (finding above).
- **Closed by removal.** The daemon-image finding is closed because that story left the set.
- **No finding.** story:session-containment-retries-an-unproven-kill and story:directory-listing-survives-special-entries have none.

What I could not establish:
- I did not run `scripts/delegated-lane.sh`, so I cannot say whether ten clean runs is achievable. Its tail does exit 0: `quota-lane --inventory` is documented to exit 0. The ten runs also include the SDK, MCP and TLS suites, not only `runtime_vectors`.
- Out of my lane (design): story:the-daemons-detached-containment-task-is-tracked removes the spawn that story:session-containment-retries-an-unproven-kill relies on for its first containment attempt on the failed-upgrade path. The case that story proves for that caller will need to drive the sweep once the second story lands.

```findings
- file: .engineering/planning/story/runtime-vector-cgroup-teardown-is-flaky.md
  line: 36
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "outcome 2 fires only on \"a process still in the per-test group\", but the Notes name a second cause (\"or a child group still exists\") and the Context records a second symptom (500 operation.outcome-unknown on exec 53), so an investigation that finds either fits neither outcome, and outcome 1 (\"fix confined to the test harness\") would admit a wait for the daemon's child group, which outcome 2 forbids"
- file: .engineering/planning/story/the-daemons-detached-containment-task-is-tracked.md
  line: 42
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the acceptance's \"Non-test ... contains no tokio::spawn\" has no stated check, and the Context's own command grep -rn tokio::spawn crates/substrate-daemon/src/app/ also returns eight hits in app/tests.rs (a cfg(test)-only file, app.rs:28) that the Context omits when it lists sessions.rs:1341 plus four in metrics.rs, so two readers can get different answers; the body must name the exclusions (app/tests.rs and metrics.rs from line 415)"
```
