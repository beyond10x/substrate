---
format: aep.planning-md/3
id: review-result:wave-2026-10-14-scope-round-2
kind: review-result
status: active
title: scope critic (aep:plan-critic-scope), wave 2026-10-14 round 2
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

Round 1's four findings are resolved, and the revisions add no new scope finding. I read the six stories, the round-1 review-result and the code at every cited site.

**Round 1 findings**
- **Detached task, JoinSet versus maintenance owner.** Resolved. The Decision now makes the maintenance sweep the owner and says no `JoinSet` is added. The acceptance no longer names a `JoinSet`. Every live site it cites checks out. That covers the sweeper spawn at `runtime.rs:639`, the aborts at `:651`, `:661` and `:729`, and the three connection `JoinSet`s at `:673`, `:1015` and `:1099`. It also covers `sessions.rs:1341` and `service.rs:293` (`nudge_maintenance`).
- **Daemon image, stale summary and title.** Resolved. The frontmatter summary and the title now state the README-statement outcome, grounded in `Dockerfile:48-59`.
- **Daemon image, inclusion rule.** Resolved. The Parallel work section says why the story is outside the wave: a documentation-only statement fixes neither correctness nor containment. It is no longer in the set.
- **Directory listing, "device" in the title.** Resolved. The title is now "FIFO, socket or non-UTF-8 name". The Context says devices fall into the shared `_ =>` arm and cannot be created unprivileged. I confirmed that arm at `fs.rs:776` and `fs.rs:819`.

**Coverage.** The selection rule yields 4 stories in 3 units, and all 4 are claimed once. The retry story and the detached-task story are one unit, and the detached-task story `depends_on` the retry story. The three excluded candidates are named as excluded. The daemon-image story is draft and outside the set, and says so itself. There is no gap, no overlap and no narrowed promise.

**Does the directory-listing Decision stay within "no successor bundle"?** Yes.
- `DirectoryEntryKind` still has three members and `DirectoryPage` has the same fields (`crates/substrate-wire/src/lib.rs:1234` and `:1038`).
- `truncated` in `workspace-tree-read-v2.json` (line 42) is a bare `{"type": "boolean"}`, so reusing it for an omission changes no schema.
- The v1 page gets no signal, and the story records that as an accepted trade-off.
- The machine-readable signal is split into `story:directory-listing-names-omitted-entries`. That draft says it needs an ADR or design document, a successor bundle and a wait for the next bundle, so it claims nothing the set claims.

**What I read.** 6 stories (the 4 in the set, the daemon-image story and the new follow-up draft) plus `review-result:wave-2026-10-14-scope-round-1`. I ran `aep plan artifact show` on each, and used `rg`, `sed` and `grep` on `runtime.rs`, `service.rs`, `sessions.rs`, `fs.rs`, the wire crate, the 0.18.0 schemas and `website/docs/reference/contract.md`. The parent is the selection rule you gave me, with 7 named candidates. All 7 are traced: 4 claimed by a story and 3 excluded on stated grounds.

**What I could not establish**
- **Out of my lane, `story:the-daemons-detached-containment-task-is-tracked`.** Its Context says `grep -rn tokio::spawn crates/substrate-daemon/src/app/` returns `sessions.rs:1341` plus four in `metrics.rs`. The grep also returns seven hits in `app/tests.rs`, which are test code, so the "non-test" acceptance still holds. The acceptance lane may want the check to say it excludes `tests.rs` too.
- **Out of my lane, `story:directory-listing-survives-special-entries`.** It labels its Decision "recorded before code, invariant 8", but the story body is the only record. Whether a story body satisfies invariant 8 for a changed meaning of a published `truncated` is for the design critic. I could find no design document or ADR in the repository that pins the current refusal of special entries.
- **Not reproduced.** I did not run any of the cited behaviour, including the FIFO refusal. This is code reading only.
- **Cosmetic.** `story:daemon-image-serves-exec-or-says-it-cannot` still has the old H1 ("The daemon image serves exec or says it cannot") under its updated title. It is outside the set.

```findings
[]
```
