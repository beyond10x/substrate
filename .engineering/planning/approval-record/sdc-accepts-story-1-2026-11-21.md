---
format: aep.planning-md/3
id: approval-record:sdc-accepts-story-1-2026-11-21
kind: approval-record
status: active
title: Stakeholder (agent:sdc) accepts story 1, the executable allow-list on linux (2026-11-21)
tags:
- sdc
relations:
- decides: story:linux-executable-allow-list
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-11-21T12:27:21Z", actor: "human:timo", revision: 2}
---
# Stakeholder (agent:sdc) accepts story 1, the executable allow-list on the `linux` adapter

## Who decided

Session `sdc` (agent:sdc), not a person, on 2026-11-21, as the stakeholder of
`story:linux-executable-allow-list`, under
`approval-record:stakeholder-decides-for-the-operator-2026-11-21`.

## Its words

> Accepted as stakeholder: story 1 (executable allow-list on the linux adapter), PR 241 at
> `611d56964`, on what you report: 19 of 19 hosted checks, three adversary and security rounds
> with D28 and D29 closed and their cases permanent in the lane, design 35 in its eighth revision
> with the six stated limits.
> For the SDC's use the limits are acceptable as stated (the list stops execve by path, not code
> inside allowed programs; the link limit, so exec lists name the directory a tool really lives
> in; a listed start with a controlling terminal is refused). The four environments nobody ran
> (Landlock ABI 1, xfs or btrfs, macOS with a list, a registered binfmt_misc handler) stay named
> in the story as not run. The merge is host-operator's; start story 2 with K2 and K8 through the
> driver as ruled.

Earlier the same day it had no objection to D28 and to D29, and added two points to D29 (the
`PT_INTERP` test on the bytes that are executed, and the measured case as a permanent lane test);
both are in the merged code.

## The merge

Session `host-operator` ordered it for the operator
(`approval-record:host-operator-decides-for-the-operator-2026-11-21`): pull request 241 at head
`611d56964`, merge commit `fcf669ee1`, with 19 of 19 hosted checks green. No release was made.

## What follows

- `story:linux-executable-allow-list` moves to `implemented`. Its body names the four
  environments nobody ran.
- `story:executable-deny-list` starts; its first failing test is K2 and K8 through the driver
  (`approval-record:sdc-accepts-d25-with-conditions-2026-11-21`).
