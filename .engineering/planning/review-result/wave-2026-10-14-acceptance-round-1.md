---
format: aep.planning-md/3
id: review-result:wave-2026-10-14-acceptance-round-1
kind: review-result
status: active
title: acceptance critic (aep:plan-critic-acceptance), wave 2026-10-14 round 1
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
story:the-daemons-detached-containment-task-is-tracked — the acceptance requires "the same JoinSet that owns the connection tasks", but the Notes leave a maintenance-owned set open and `runtime.rs` holds three connection JoinSets (lines 673, 1015, 1099), so an implementor can pick either and nobody can say which one passes — .engineering/planning/story/the-daemons-detached-containment-task-is-tracked.md:33
story:runtime-vector-cgroup-teardown-is-flaky — the acceptance names no outcome for the branch where the investigation finds the daemon leaves the group populated, so ten green runs cannot be reached by this story and it can be neither done nor closed there — .engineering/planning/story/runtime-vector-cgroup-teardown-is-flaky.md:33
story:directory-listing-survives-special-entries — the acceptance names only the two routes' answers, so it passes with `website/docs/reference/contract.md` unchanged even though the Decision commits to stating the v1 omission there — .engineering/planning/story/directory-listing-survives-special-entries.md:47
story:daemon-image-serves-exec-or-says-it-cannot — "names what a deployment of that image must still supply" does not fix the list the Context gives (delegated cgroup subtree, process-free root, `--cgroup-root`, a runtime that permits bubblewrap's namespaces), so a README naming one of them passes — .engineering/planning/story/daemon-image-serves-exec-or-says-it-cannot.md:36

What I read: all 5 ids given and read, 5 of 5, with `aep plan artifact show` for each. I also ran `aep plan artifact kinds` and `aep plan artifact lifecycle story`. In the tree I read the `README.md` § Serving exec, the `Dockerfile` stages, `sessions.rs` around the spawn, `runtime.rs`, `fs.rs` (`list_directory` and `walk_tree`), the pinned case in `tests/pipe_session.rs` and the tail of `scripts/delegated-lane.sh`.

What I could not establish:
- I did not run `scripts/delegated-lane.sh`, so I cannot say whether ten clean runs is achievable.
- Out of my lane (accuracy and design): the Context of `story:the-daemons-detached-containment-task-is-tracked` cites `runtime.rs:621` as the JoinSet, but line 621 is in `serve()` and not a JoinSet.
- Out of my lane: `story:session-containment-retries-an-unproven-kill` cites `runtime.rs:534` as the 250 ms sweep tick. I did not find the sweep tick there.
- Out of my lane: the stored summary and title of `story:daemon-image-serves-exec-or-says-it-cannot` still describe the superseded distroless premise.
- `story:session-containment-retries-an-unproven-kill` has no finding. Its acceptance is one observable sentence and names the before-state: the pinned case `a_stranded_attach_whose_kill_is_unproven_is_only_ended_by_lease_expiry` at `crates/substrate-daemon/tests/pipe_session.rs:2531`.

```findings
- file: .engineering/planning/story/the-daemons-detached-containment-task-is-tracked.md
  line: 33
  category: acceptance
  severity: blocker
  verdict: needs-revision
  origin: introduced
  message: "the acceptance requires \"the same JoinSet that owns the connection tasks\", but the Notes leave a maintenance-owned set open and runtime.rs holds three connection JoinSets (lines 673, 1015, 1099), so an implementor can pick either and nobody can say which one passes"
- file: .engineering/planning/story/runtime-vector-cgroup-teardown-is-flaky.md
  line: 33
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the acceptance names no outcome for the branch where the investigation finds the daemon leaves the group populated, so ten green runs cannot be reached by this story and it can be neither done nor closed there"
- file: .engineering/planning/story/directory-listing-survives-special-entries.md
  line: 47
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the acceptance names only the two routes' answers, so it passes with website/docs/reference/contract.md unchanged even though the Decision commits to stating the v1 omission there"
- file: .engineering/planning/story/daemon-image-serves-exec-or-says-it-cannot.md
  line: 36
  category: acceptance
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "\"names what a deployment of that image must still supply\" does not fix the list the Context gives (delegated cgroup subtree, process-free root, --cgroup-root, a runtime that permits bubblewrap's namespaces), so a README naming one of them passes"
```
