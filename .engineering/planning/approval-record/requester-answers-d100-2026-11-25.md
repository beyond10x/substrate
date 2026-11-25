---
format: aep.planning-md/3
id: approval-record:requester-answers-d100-2026-11-25
kind: approval-record
status: active
title: The requester's answer on starting the oci memory-limit story (2026-11-25)
summary: 'D100 = A: the story starts; one measurement job on a standard hosted runner, design 36 D7 amended before code, the gate''s case stays'
relations:
- decides: story:an-oci-run-killed-at-its-memory-bound-names-the-limit-on-a-hosted-runner
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-11-25T16:29:13Z", actor: "agent:substrate", revision: 2}
---
# The requester's answer on starting the oci memory-limit story (2026-11-25)

## Who decided

The requester of `epic:runs-are-governed-and-limit-hits-are-events`, the stakeholder
`story:an-oci-run-killed-at-its-memory-bound-names-the-limit-on-a-hosted-runner` names, under the
rule that the stakeholder of a request decides for the work it requested (`AGENTS.md`
§ *Planning artifacts*).

## The question

Asked on 2026-11-25 as decision D100, after the third sighting of the failure, with option A as
the default:

- A: move the story from `draft` to `proposed` and `active`. Step 1 is one measurement job on a
  hosted runner from a branch that is not merged: the time from the exit to the runtime's
  `OOMKilled` flag, and the count of runs with no flag. Then the amendment of design 36 D7,
  before code. The gate's case stays as it is until the fix.
- B: keep the story a draft and run the gate job again on each sighting.
- C: start the story and take the gate's case out of the gate until the fix.

## The answer

> D100 = A. Start it; measure first on a standard hosted runner (no larger runner: that costs
> money and needs Timo's yes), amend design 36 D7 before code, keep the gate's case in the gate.

## What was read before the question

Three sightings of `an_oci_run_over_its_memory_bound_names_the_memory_limit` on a hosted runner,
each a run that ended with signal 9 and `limit: None`:

- 2026-11-24, recorded in design 36 under what is not known; the monitor then made one read of
  the runtime's state;
- 2026-11-25, run 45594091184, attempt 1, job 139144273381, pull request 302: the run ended
  after 1.61908603 s;
- 2026-11-25, run 45644944907, attempt 1, job 139325324759, the push of `main` at `224e1e5ffd`:
  the run ended after 1.466541576 s.

The second and third came after the monitor read the state again for one second. Neither
changed code of the host crate.

## What follows

- The story moves to `proposed` and `active`.
- The measurement runs on a standard hosted runner, never a larger one.
- The gate's case keeps its assertion and stays in the gate.
