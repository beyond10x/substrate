---
format: aep.planning-md/3
id: approval-record:run-governance-first-slice-accepted
kind: approval-record
status: active
title: Operator accepts the run-governance epic and its first slice (2026-11-21)
tags:
- sdc
relations:
- decides: epic:runs-are-governed-and-limit-hits-are-events
- decides: story:linux-executable-allow-list
- decides: story:executable-deny-list
- decides: story:run-observation-names-the-limit-that-ended-it
- decides: story:audit-records-run-end-and-limit
- decides: story:process-count-limit-hit-is-observed
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-11-21T02:34:00Z", actor: "human:timo", revision: 2}
---
# Operator accepts the run-governance epic and its first slice

## Decision

On 2026-11-21 the substrate session put one question to the operator, Timo, in that session:

> Substrate: the 5 Loom gaps are confirmed at main adfde4020 and planned as 1 epic + 9 draft
> stories (2 critic rounds, store valid), uncommitted on branch plan/sdc-run-governance.
> Committing and accepting stories need your word. What do I do with the plan?

He chose the option labelled "Commit + PR + accept slice 1", whose description read:

> Commit as bot, push, open a PR. Move the epic and the 5 in-process linux stories (allow-list,
> deny-list, limit on observation, run_ended record, process-count hit) to active. I then start
> story 1 with its design document 35. Recommended: the SDC prototype needs these 5 first.

## What it covers

- `epic:runs-are-governed-and-limit-hits-are-events` moves to active.
- These five stories move to active, in this order of work:
  1. `story:linux-executable-allow-list`
  2. `story:executable-deny-list`
  3. `story:run-observation-names-the-limit-that-ended-it`
  4. `story:audit-records-run-end-and-limit`
  5. `story:process-count-limit-hit-is-observed`
- The plan is committed on branch `plan/sdc-run-governance` and opened as a pull request.
- Work starts with story 1's design document, `docs/design/35-executable-lists.md`.

## What it does not cover

- The four other stories of the epic stay draft: `story:executable-lists-on-oci-and-pod`,
  `story:exec-start-carries-executable-lists`,
  `story:exec-observation-reports-process-count-hits` and
  `story:confinement-profile-ingress-routes`.
- Merging the pull request, any release, tag or version number.
- The answers session `sdc` gave on 2026-11-21 (in-process `linux` first, summed limits stay in
  Loom, a deny-list always comes with an allow-list). The epic records them as that session's own
  and reversible.

## State of the critic panel at the decision

Two rounds of the four plan critics ran. Round 1 returned 19 findings and round 2 returned 8; all
27 are recorded as `fixed`. The fixes made after round 2 were not read by a critic again, because
the panel stops after its second round.
