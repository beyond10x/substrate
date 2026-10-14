---
format: aep.planning-md/3
id: review-result:wave-2026-10-14-parallel-safety-round-1
kind: review-result
status: active
title: parallel-safety critic (aep:plan-critic-parallel-safety), wave 2026-10-14 round 1
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
story:directory-listing-survives-special-entries — it and story:session-containment-retries-an-unproven-kill (units 3 and 1) both change shipped behaviour, and this repo records such a change as an entry under `## [Unreleased]` in `CHANGELOG.md`. Neither body names that file, and both entries would be inserted at the same hunk (inferred, from commits 25ae3f93, d9391db5 and cbfd183c, which each added changelog text in the same region). The "No other unit touches those files" sentence is silent on it. A revision should add an ordering edge or a merge-order note recording `CHANGELOG.md` as the reason, or have each unit hand its entry to the integrator — `~/.local/state/worktree/trees/b10x/substrate/w15-plan/.engineering/planning/story/directory-listing-survives-special-entries.md:53`, `~/.local/state/worktree/trees/b10x/substrate/w15-plan/CHANGELOG.md:8`

**What I found**
- The one file that two of the five stories land on is `crates/substrate-daemon/src/app/sessions.rs`. The pair is `story:session-containment-retries-an-unproven-kill` and `story:the-daemons-detached-containment-task-is-tracked`.
- The plan says so in both bodies: one implementor, in sequence, retry first. That collision is inside unit 1 and is not a finding.
- No file collision exists across units 1, 2, 3 and 4 by their cited scopes.
- The only unnamed cross-unit shared file I found is `CHANGELOG.md` (the finding above).
- The `README.md` change (unit 4) and the `website/docs/reference/contract.md` change (unit 3) are on different files. README is not a website page, and no checker or test reads both. `user_namespace_floor_documents.rs` reads only README.
- Unit 2 lands on `crates/substrate-daemon/tests/runtime_vectors.rs`, and on `scripts/delegated-lane.sh` only if the race is there. Unit 1's test files are `pipe_session.rs` and, for the tracking story, probably `crates/substrate-daemon/src/app/tests.rs`, which is inferred and also inside unit 1. None of the three test files imports a shared test-helper module.
- No unit needs a new dependency, so `Cargo.toml` and `Cargo.lock` stay untouched. `libc` is already used in `crates/substrate-host/src/fs.rs` tests (line 1868).

What I read: 5 stories, plus the aep rubric and the parallel-safety procedure. I ran `aep plan artifact show` on each story, `aep plan artifact waves --status draft`, `aep plan artifact graph` and `aep plan artifact relations`, and used `git grep` and `git log` on `CHANGELOG.md`, the test files, `xtask` and the Dockerfile references.

What I could not establish:
- Whether the tracking story's shutdown test goes in `runtime.rs`'s test module or in `app/tests.rs`. Either is inside unit 1, so the collision picture does not change.
- Whether units 2 and 4 owe a changelog entry. I treated them as not owing one, because they are test-harness and README-only changes.
- Surfaces cited / inferred / unplaceable: 5 / 0 / 0. The `CHANGELOG.md` finding is inferred and is not in those five.
- Acceptance reads traced to a producer in the set: 0 (0 recorded by a `depends_on` edge). No story's acceptance reads state another story in the set creates.
- No `depends_on` edge exists between the two unit-1 stories. Their order is stated in prose, and the operator decides whether it needs an edge. Unit 1's retry-versus-tracking order is not mine to rule on.
- Whether the split between the two unit-1 stories is right is the design critic's lane.
- The collisions `aep plan artifact waves` reports against stories outside this set are out of scope.

```findings
- file: .engineering/planning/story/directory-listing-survives-special-entries.md
  line: 53
  category: parallel-safety
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "this story and story:session-containment-retries-an-unproven-kill (units 3 and 1) both change shipped behaviour and so both add an entry under `## [Unreleased]` in CHANGELOG.md, at the same hunk (inferred from commits 25ae3f93, d9391db5 and cbfd183c), and neither body names that file or orders the two"
```
