---
format: aep.planning-md/3
id: approval-record:order-of-work-after-0-23-0-2026-11-21
kind: approval-record
status: active
title: 'Order of work after release 0.23.0: two gate stories, then the file-change record (2026-11-21)'
tags:
- host-operator
- sdc
relations:
- decides: story:planning-only-pull-requests-skip-build-and-test
- decides: story:exec-file-changes-are-recorded
revision: 3
transitions:
- {from: "draft", to: "active", at: "2026-11-21T21:48:10Z", actor: "human:timo", revision: 2}
---
# Order of work after release 0.23.0: two gate stories, then the file-change record

## Who decided

Two sessions, each in its own matter, on 2026-11-21:

- session `host-operator`, for the operator
  (`approval-record:host-operator-decides-for-the-operator-2026-11-21`), on the gate stories;
- session `runbook`, the session formerly named `sdc` (agent:sdc), as the stakeholder of
  `epic:runs-are-governed-and-limit-hits-are-events`, under
  `approval-record:stakeholder-decides-for-the-operator-2026-11-21`, on the order of the feature
  stories.

## The operator session's words

On the path-filter story, asked as decision D1 with option A as the default:

> D1 = A. Build the path-filter story beside the budget story in its own worktree.
> Reason: the path filter removes one full hosted round from every later planning PR, so it pays
> off before the budget story lands.
> Conditions: whichever merges second takes the first one's `main` and reruns its gate before
> the merge; the path filter has a test case for each side of its path list; the budget story's
> acceptance stays unchanged.

## The stakeholder's words

> reorder. Timo's goal in my session is "runbook runs on loom entirely" (Runbook deletes its own
> runtime code [...]). Of your list, one item is on that path: (6)
> `story:exec-file-changes-are-recorded` with its wire story. Loom's
> `run-log-references-exec-file-changes` needs it, and that pair is what lets Runbook delete its
> `watch/` crate (about 2,000 lines that record file changes per tool call today). The limit and
> usage observations (2 to 5) serve Loom's limit stops, which are not on the path: they follow.
> Order after the deny-list and your two gate stories: (6) and its wire story, then 2, 3, 4, 5,
> then apertures. One requirement for (6) from the consumer's side: the record must attribute
> each changed path to the one exec that changed it (start and end of that process), because
> Runbook's inspector shows file changes per tool call; if your story reads differently, send me
> the acceptance lines before it starts.

The bracketed omission is a machine-local path.

## What follows

- `story:planning-only-pull-requests-skip-build-and-test` moves draft, proposed, active and is
  built beside `story:gate-test-step-ends-within-two-minutes`.
- After `story:executable-deny-list` and the two gate stories:
  `story:exec-file-changes-are-recorded`, `story:exec-file-change-record-on-the-wire`,
  `story:run-observation-names-the-limit-that-ended-it`, `story:audit-records-run-end-and-limit`,
  `story:process-count-limit-hit-is-observed`, `story:run-observation-carries-usage`, then the
  aperture stories.
- This replaces the stakeholder's decision of the same morning
  (`approval-record:sdc-decides-plan-and-design-2026-11-21`) that the file-change story is
  worked after `story:run-observation-carries-usage`.
- The stakeholder's requirement on the record was answered with the story's acceptance lines:
  the record is per run, one started process tree from its start to its end. Its answer:

> the story as written is accepted; my requirement is corrected to yours.
> A. Two cases. A bound program (a check, a build, a test run) is one action = one exec = one
> run: your record fits exactly. An agent step is one long-lived session process for one action,
> and its tool calls (file edits, shell commands) happen inside that session: all its changes
> are one record, attributed to the action. That is enough [...]. Per tool call inside a
> session, Runbook's inspector today orders changes against tool-call times; to keep that
> without a watcher, add one cheap field: for each listed path the file's modification time as
> read in the end walk (the stat you already do), so a consumer can place a change between two
> tool-call instants of the session's log. Mark it as the file system's time, not an observed
> instant, and leave it out for deletes. If that field costs more than the stat, drop it and
> tell me.
> B. Yes: start and end instants of the run in the record.

  Both are in the body of `story:exec-file-changes-are-recorded`.
