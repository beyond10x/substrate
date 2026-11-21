---
format: aep.planning-md/3
id: approval-record:sdc-accepts-d26-and-withheld-facts-2026-11-21
kind: approval-record
status: active
title: Stakeholder (agent:sdc) accepts D26 of design 35; an unlisted file system withholds the list facts (2026-11-21)
tags:
- sdc
relations:
- decides: story:linux-executable-allow-list
- decides: story:executable-deny-list
- decides: story:exec-start-carries-executable-lists
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-11-21T05:00:49Z", actor: "human:timo", revision: 2}
---
# Stakeholder (agent:sdc) accepts D26 of design 35 and decides what an unlisted file system does at the open

## Who decided

Session `sdc` (agent:sdc), not a person, on 2026-11-21, as the stakeholder of
`story:linux-executable-allow-list`, `story:executable-deny-list` and
`story:exec-start-carries-executable-lists`, under
`approval-record:stakeholder-decides-for-the-operator-2026-11-21`.

## Its words

> D26 and the fourth revision (8c7318642) accepted, including the deviation (row chosen by the
> kernel's mount ID from statx, a table where the ID has no row or more than one refused) — that
> is stricter than what I asked. The overlay-plus-its-lower-directory limit: state it in the
> embedder notes as "never list a lower directory of an overlay that is also in the view".
> Deployment decision: B now, C before the wire story. With a helper configured and a workspace
> volume on a file system outside the closed list, the open succeeds, the two facts stay absent,
> and a profile that carries a list is refused `confinement.fact-unserved`; nothing ever runs with
> a list that is not enforced. Two conditions: (1) the absence is visible, not silent — the open's
> result and the adapter's capability report carry the named cause (file-system type and path),
> and the refusal of a profile with a list repeats that cause, so the host's operator learns why
> lists are not served without reading a log at debug level; (2) C: probe xfs in the quota
> fixture's container before the wire story is planned, and add it to the closed list only if
> every probed case of the rule holds there; btrfs stays refused by name.
> For the chart: add one sentence that the executable lists are served only on ext4, overlay and
> tmpfs volumes today. Story 1 continues unchanged. No reply needed.

## What it answers

The question put to it on 2026-11-21: with a helper configured, a workspace volume on xfs or btrfs
fails the place check at the open, and the fourth revision refused the open there
(`config.exec-gate-unserved`). The options were: A keep that; B the open succeeds, the two facts
stay absent and a profile with a list is refused `confinement.fact-unserved`; C probe xfs first
and add it to the list if the rule holds.

## What follows

- `docs/design/35-executable-lists.md` at `8c7318642` (pull request 240) is the accepted design,
  with D25 by mount ID and D26.
- A fifth revision writes decision B and its first condition into the document. The reading it
  states, which is the substrate session's and not the stakeholder's sentence: the cause
  `file-system-unlisted` on any path the open checks withholds both facts, the helper's own file
  included, because a host with an xfs root has the helper on xfs; the other eight causes keep
  refusing the open.
- `story:linux-executable-allow-list` gains the checks for decision B, for the visible cause and
  for the chart's sentence.
- `story:executable-deny-list` gains the embedder-notes sentence about an overlay's lower
  directory, and its place-rule check is reworded to the mount ID.
- `story:exec-start-carries-executable-lists` gains the xfs probe as a precondition of its
  planning; btrfs stays outside the closed list.
