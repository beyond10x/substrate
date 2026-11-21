---
format: aep.planning-md/3
id: approval-record:sdc-accepts-design-35-2026-11-21
kind: approval-record
status: active
title: Stakeholder (agent:sdc) accepts design document 35 (2026-11-21)
tags:
- sdc
relations:
- decides: story:linux-executable-allow-list
- decides: story:executable-deny-list
- decides: story:exec-start-carries-executable-lists
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-11-21T04:03:11Z", actor: "human:timo", revision: 2}
---
# Stakeholder (agent:sdc) accepts design document 35

## Who decided

Session `sdc` (agent:sdc), not a person, on 2026-11-21, as the stakeholder of
`story:linux-executable-allow-list` and `story:executable-deny-list`, under
`approval-record:stakeholder-decides-for-the-operator-2026-11-21`.

## Its words

> accepted. Design document 35 at ab22126ff is accepted by me as the stakeholder for points 1 to 5
> and the rest of the review; start story:linux-executable-allow-list with an implementor in its
> own worktree, failing test first. A confirming read of the revision by the same reviewer runs in
> parallel on my side; anything it finds reaches you as a correction before story 1 merges and
> does not hold the start.
> The second gap your probes found (one directory mounted twice, `--alias`) is the kind of result
> I want kept visible: keep both lane cases (sh → bash, and /usr/bin listed with /bin/id denied)
> as permanent delegated-lane cases, not only design probes.
> Plan consequence: agreed — until story:explicitly-empty-exec-subpaths-means-the-program-only
> lands, the daemon refuses a present-and-empty wire list by a named refusal, so no accepted byte
> changes meaning later. Put that into story:exec-start-carries-executable-lists as an acceptance
> line.
> Merge of PR #240 and anything on main stay the operator's.

## What it covers

- `docs/design/35-executable-lists.md` at commit `ab22126ff` (pull request 240) is the accepted
  design for both stories. Invariant 8's "design before code" is met by this acceptance.
- Work on `story:linux-executable-allow-list` starts, on a branch stacked on the design branch,
  so its code cannot reach `main` before the document does.
- `story:executable-deny-list` gains the two lane cases as permanent delegated-lane cases.
- `story:exec-start-carries-executable-lists` gains the refusal of a present and empty wire list
  as a check.

## What it does not cover

- Merging pull request 240 or anything else into `main`: the operator's.
- A correction from session `sdc`'s confirming read, which arrives before story 1 merges.
