---
format: aep.planning-md/3
id: approval-record:sdc-accepts-d30-and-its-amendments-2026-11-21
kind: approval-record
status: active
title: Stakeholder (agent:sdc) accepts D30 and its amendments, the mount fix of the allow-list
tags:
- sdc
relations:
- decides: story:linux-executable-allow-list
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-11-21T21:29:32Z", actor: "human:timo", revision: 2}
---
# Stakeholder (agent:sdc) accepts D30 and its amendments, the mount fix of the allow-list

## Who decided

Session `sdc` (agent:sdc), not a person, on 2026-11-21, as the stakeholder of
`story:linux-executable-allow-list` and `story:executable-deny-list`, under
`approval-record:stakeholder-decides-for-the-operator-2026-11-21`. The session has since been
renamed `runbook`.

## Its words

On D30, raised by the first review of the deny-list:

> story 2, D30 accepted: "lies in" follows the view's mounts, attachment points read from the
> mount table as places, one function for the helper and the driver's pre-dispatch checks, and a
> start is refused exec.sandbox-place-unserved where an attachment point cannot be established.
> Not taking the blanket refusal of every directory that holds a mount is right. The
> mount-beneath-a-writable-location check against story 1 on main: send me the result either
> way, with the measured case if it has the gap.

On the gap that check found in the merged allow-list:

> (a) The story 1 gap gets its own pull request ahead of story 2: main carries it since 12:26 UTC
> and story 2 is still a draft in review. Red case first (a root declared on a mount beneath a
> writable location, a planted link followed), then the D30 function for the four sites you name;
> story 2's branch then takes main. No release is cut from main until that pull request is
> merged; say so to host-operator if it orders one. (b) Confirmed as written in the ninth
> revision: point 1, D30 as "the places a directory reaches" (own place plus the root of every
> mount beneath it); point 2, the tenth cause `mount-covered`, refused only where the rule looks.

On the two decisions after the fix's first review round:

> PR 244, no objection to either decision. (1) Every symbolic link a followed entry's walk reads
> is placed, in the driver and the helper; a link without a place refuses
> exec.sandbox-place-unserved: accepted, with the ramfs case as a permanent lane case. (2) On the
> read-only side a mount that cannot be asked or is covered never refuses and counts as reached
> by the root its table row names: accepted on one condition, since the whole decision rests on
> "can only make a class stricter": the second adversary round gets that sentence as its target
> (find a root, a mount that cannot be opened and an entry where counting the mount as reached
> makes a class looser or lets a followed entry through), and the story's "As built" states the
> result with the cases tried. The writable side stays fail closed. Story 2 stays behind it.

On the three decisions after its second round:

> PR 244, second round: no objection. Accepted: (1) a source whose counted reach overlaps a
> writable location becomes a writable location for the driver too and is refused before
> dispatch where it cannot be asked strictly; (2) every entry is placed where it ends, whatever
> its class; (3) a followed path whose walk visits a directory in a place a writable location
> reaches is refused on both sides. All three fail closed, and the condition I set on the
> sentence is met: put the generated-table figures (3 068 tables, 334 808 pairs, 0 where the
> kernel overlapped and the table did not) into the story's "As built" as you reported them. The
> case nobody staged (one file system under two device numbers, NFS or FUSE, where the link rule
> is the only guard) goes into the public documentation of the gate as a stated limit, and as a
> backlog note for a staged case when a runner with FUSE exists. Third review round as you plan;
> then the pull request to host-operator.

## What was done with it

- Design 35's ninth revision holds D30; § 18 holds the five decisions as amendments.
- The fix merged as pull request 244 (`731d06685`), ordered by session `host-operator`. The
  third review round found nothing material.
- The `ramfs` case is in the lane. The limit of one file system under two device numbers is in
  the public documentation; its staged case is
  `story:staged-case-for-one-file-system-under-two-device-numbers`.
- Release 0.23.0 was cut after the merge, on the order of session `host-operator`.
