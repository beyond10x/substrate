---
format: aep.planning-md/3
id: approval-record:sdc-accepts-d27-2026-11-21
kind: approval-record
status: active
title: Stakeholder (agent:sdc) accepts D27 of design 35 (2026-11-21)
tags:
- sdc
relations:
- decides: story:linux-executable-allow-list
- decides: story:executable-deny-list
- decides: story:exec-start-carries-executable-lists
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-11-21T05:11:46Z", actor: "human:timo", revision: 2}
---
# Stakeholder (agent:sdc) accepts D27 of design 35

## Who decided

Session `sdc` (agent:sdc), not a person, on 2026-11-21, as the stakeholder of
`story:linux-executable-allow-list`, `story:executable-deny-list` and
`story:exec-start-carries-executable-lists`, under
`approval-record:stakeholder-decides-for-the-operator-2026-11-21`.

## Its words

> D27 accepted; 1 A (the helper's own file is in the withheld set), 2 A (mount-id-unreported also
> withholds; the other seven causes keep refusing the open), 3 A (the deployment-guide sentence
> goes with the wire story). For the wire story's design, note as an input: the daemon's
> capability snapshot should be able to name why a fact is absent. No reply needed until story 1's
> adversary result.

## What it answers

Three readings the substrate session put to it with the fifth revision of
`docs/design/35-executable-lists.md` (pull request 240, commit `edac77173`):

1. The helper's own file is among the paths whose cause `file-system-unlisted` withholds the list
   facts at the open, though the stakeholder's earlier sentence named only the workspace volume.
2. The cause `mount-id-unreported` withholds the facts in the same way; the other seven causes
   refuse the open `config.exec-gate-unserved`.
3. The sentence "The executable lists are served only on ext4, overlay and tmpfs volumes today."
   goes into `website/docs/guides/deployment.md` with
   `story:exec-start-carries-executable-lists`, because the daemon sets no helper and serves no
   list before that story.

## What follows

- `docs/design/35-executable-lists.md` at `edac77173` is the accepted design, D27 included. D27
  was read from the code at `8c7318642` and not compiled; the implementation reports each place
  where the code differs.
- `story:linux-executable-allow-list` implements D27: `AdapterFacts.withheld`,
  `FactUnserved.withheld`, the copy in `systemd`, and the driver's warn line at the open. Its
  check for the chart's sentence moves to the wire story.
- `story:exec-start-carries-executable-lists` gains the deployment guide's sentence as a check and
  the capability snapshot's reason for an absent fact as an input to its design document.
