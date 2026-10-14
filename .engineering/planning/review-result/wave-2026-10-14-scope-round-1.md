---
format: aep.planning-md/3
id: review-result:wave-2026-10-14-scope-round-1
kind: review-result
status: active
title: scope critic (aep:plan-critic-scope), wave 2026-10-14 round 1
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

story:the-daemons-detached-containment-task-is-tracked — the acceptance requires "the same `JoinSet` that owns the connection tasks" (via a stale `runtime.rs:621` cite), while its Notes offer a "maintenance-owned" set that the acceptance rules out. Pick one and cite a live site; `runtime.rs` now holds three accept-loop `JoinSet`s, at lines 673, 1015 and 1099. — .engineering/planning/story/the-daemons-detached-containment-task-is-tracked.md:33
story:daemon-image-serves-exec-or-says-it-cannot — the frontmatter summary still says "the distroless image (Dockerfile:12) has neither; inferred from the base image", and the title still offers "or documents that it serves no exec". The revised body says the daemon stage is Ubuntu with bwrap and socat (`Dockerfile:48-59`). Update summary and title to the README-statement outcome. — .engineering/planning/story/daemon-image-serves-exec-or-says-it-cannot.md:7
story:daemon-image-serves-exec-or-says-it-cannot — the revised outcome is a README statement about an image that already carries the prerequisites. That fixes neither correctness nor containment, and it is not among the named candidates, so it does not meet the wave's inclusion rule. Either state why a documentation-only story belongs in this wave or leave it a draft outside it. — .engineering/planning/story/daemon-image-serves-exec-or-says-it-cannot.md:34
story:directory-listing-survives-special-entries — the title promises "device" entries, but the Context, the Decision and the acceptance test cover only FIFO, socket and non-UTF-8 name. The `_ =&gt;` arm in `fs.rs` is shared, but the story does not say a device is covered by it or that a test cannot create one unprivileged. Drop "device" from the title or record that. — .engineering/planning/story/directory-listing-survives-special-entries.md:6

Selection judgement, with no finding raised:
- **Kill retry, cgroup flake, detached task, special entries:** each is claimed by one story and each honours its own outcome. Nothing in them overlaps.
- **Exclusions:** `read-only-roots-pinned-against-symlinks`, `transport-admission-and-stream-lifetime-disagree` and `session-start-publishes-exec-fields` were rightly excluded. Their own bodies name an ADR note or successor ADR, a design document or ADR before code, and a successor bundle (`story:read-only-roots-pinned-against-symlinks` also decomposes `epic:host-run-gates`, outside the pool). The other drafts that need an ADR, a successor bundle or a provisioned runner are also correctly left out.
- **Directory-listing decision:** it adds no wire change. The published `workspace-tree-read-v2.json` `truncated` is a bare boolean with no described meaning, so setting it on an omission does not contradict the schema.
- **Reach beyond the story:** I found none in the drafts. The diffs only narrow scope or record decisions.

What I read: 5 stories, 7 other drafts (the three excluded stories plus four others), `epic:remote-serving`, `epic:review-2026-09-03-findings`, the store's artifact list and graph, the git diff of the five edited store files, and the code at the cited sites. Commands: `aep plan artifact show/list/graph`, `git diff`, `rg`, `sed`. I extracted 7 promises from the selection request (7 named candidates) and traced all 7: 4 claimed by a story and 3 honestly excluded.

What I could not establish:
- **Retry story, stale cites (out of my lane, not part of the verdict):** `runtime.rs:534` is now line 640 and `service.rs:645` is now line 643.
- **Retry story, retry state (out of my lane):** it does not say where retry state lives, or whether it survives a restart. That could turn into an ADR-level question for the design critic.
- **Directory-listing, not reproduced:** I did not reproduce the FIFO refusal at runtime.
- **Daemon image, not pulled:** the story says no image is pulled, so bwrap and socat in the image are read from the `Dockerfile` only.

```findings
- file: .engineering/planning/story/the-daemons-detached-containment-task-is-tracked.md
  line: 33
  category: scope
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the acceptance requires 'the same JoinSet that owns the connection tasks' (citing runtime.rs:621, which is no longer a JoinSet; runtime.rs holds three accept-loop JoinSets at lines 673, 1015 and 1099) while its Notes offer a maintenance-owned set the acceptance excludes; pick one and cite a live site"
- file: .engineering/planning/story/daemon-image-serves-exec-or-says-it-cannot.md
  line: 7
  category: scope
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the frontmatter summary still says the distroless image has neither bwrap nor socat (inferred), and the title still offers 'or documents that it serves no exec', contradicting the revised body that says the daemon stage carries them (Dockerfile:48-59); update summary and title to the README-statement outcome"
- file: .engineering/planning/story/daemon-image-serves-exec-or-says-it-cannot.md
  line: 34
  category: scope
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the revised outcome is a README statement about an image that already carries the prerequisites, which fixes neither correctness nor containment and is not a named candidate, so it falls outside the wave's inclusion rule; state why it belongs in this wave or leave it a draft outside it"
- file: .engineering/planning/story/directory-listing-survives-special-entries.md
  line: 6
  category: scope
  severity: warning
  verdict: needs-revision
  origin: introduced
  message: "the title promises 'device' entries but the Context, Decision and acceptance test cover only FIFO, socket and non-UTF-8 name, with nothing saying a device is covered by the same branch or cannot be created unprivileged; drop 'device' from the title or record it"
```
