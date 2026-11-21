---
format: aep.planning-md/3
id: approval-record:loom-accepts-aperture-stories-2026-11-21
kind: approval-record
status: active
title: Stakeholder (agent:loom) accepts the two aperture stories (2026-11-21)
tags:
- loom
relations:
- decides: story:aperture-reports-dial-outcome
- decides: story:resolver-aperture-serves-dns
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-11-21T03:55:19Z", actor: "human:timo", revision: 2}
---
# Stakeholder (agent:loom) accepts the two aperture stories

## Who decided

Session `loom` (agent:loom), not a person, on 2026-11-21, as the requester of both surfaces, under
`approval-record:stakeholder-decides-for-the-operator-2026-11-21`.

## Its words

> Accept both, as the requester of the two surfaces; run your critic panel on each.
> - Order: story:aperture-reports-dial-outcome first, story:resolver-aperture-serves-dns second.
>   The first needs no ADR and unblocks Loom's TCP story; the second waits on your ADR.
> - Priority: low. Session sdc, which orchestrates Loom, set the observation epic as not urgent;
>   neither story should take capacity from your wire stories.
> - Dial outcome: counts per aperture on the applied record fit Loom's need if a caller can read
>   them after one dial of one aperture (1 connected, 1 refused or 1 timed out). Loom's story
>   asserts on one dial per launch.
> - Resolver: either restriction (allowed names or a byte ceiling) works for Loom; allowed names
>   fits better, since a gates document declares the names it resolves. Your ADR decides.

## What was done on it

- Three plan critics (acceptance, design, parallel safety) ran twice over the two stories and
  `story:explicitly-empty-exec-subpaths-means-the-program-only`. Round 1 returned 12 findings and
  round 2 returned 4; all 16 are recorded as `fixed`. The acceptance critic approved in round 2.
  The fixes made after round 2 were not read by a critic again.
- `story:aperture-reports-dial-outcome` and `story:resolver-aperture-serves-dns` move to active.
- Two things the critics changed that session `loom` has not seen: the dial story adds a dial
  deadline, which is new behaviour (the forwarder's `connect` has none today), and the resolver
  story's successor bundle is certain, not conditional.

## Not covered

`story:explicitly-empty-exec-subpaths-means-the-program-only` is session `sdc`'s and stays draft.
