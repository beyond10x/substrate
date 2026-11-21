---
format: aep.planning-md/3
id: approval-record:sdc-accepts-d25-with-conditions-2026-11-21
kind: approval-record
status: active
title: Stakeholder (agent:sdc) accepts D25 of design 35, with two conditions (2026-11-21)
tags:
- sdc
relations:
- decides: story:linux-executable-allow-list
- decides: story:executable-deny-list
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-11-21T04:35:02Z", actor: "human:timo", revision: 2}
---
# Stakeholder (agent:sdc) accepts D25 of design 35, with two conditions

## Who decided

Session `sdc` (agent:sdc), not a person, on 2026-11-21, as the stakeholder of
`story:linux-executable-allow-list` and `story:executable-deny-list`, under
`approval-record:stakeholder-decides-for-the-operator-2026-11-21`.

## Its words

> A — D25 ("the place of a path": device and root of the path's mount from /proc/self/mountinfo,
> joined with the rest of the path) is accepted in place of my ancestor rule; probe K2 shows my
> rule lets a denied file run, so it is withdrawn. Corrections 2 to 5 with your additions:
> accepted. The "list /usr/bin, not /usr" limit and its lane case: accepted; give Loom's embedder
> notes that sentence verbatim.
> Two conditions on D25, both before story 2 (the deny part) merges:
> 1. Fail closed where the place cannot be established. If a path's mount cannot be found in
> mountinfo, or two mountinfo rows could both be its mount, or the file system is one the rule has
> not been shown to hold on, the profile is refused with a named refusal; the driver never falls
> back to comparing path text. State the list of file systems the rule is shown to hold on in the
> document and in the refusal's reference text.
> 2. Probe overlayfs before story 2 merges: the daemon image runs with an overlayfs root (inferred
> from it being a container image — confirm from the Dockerfile and the hosted lane), so the rule
> must be run there, not only on ext4 with bind mounts. btrfs subvolumes (where st_dev and the
> mountinfo device differ) and network file systems may stay unprobed if condition 1 refuses them
> by name.
> The driver side of the rule being a proposal that nothing has run yet: story 2's first failing
> test should be exactly K2 and K8 through the driver, not only through the helper.

## What follows

- `docs/design/35-executable-lists.md` at `d6787264e` (pull request 240) is the accepted design,
  with D25. A fourth revision writes condition 1 into it and records the overlayfs probe.
- `story:executable-deny-list` gains both conditions and the first-test rule as checks.
- `story:linux-executable-allow-list` uses the place rule for an entry's class and for the helper
  path, so it fails closed the same way from its first merge.
- A fact found while recording this: on the workstation `/tmp` is tmpfs and `/` is ext4. Tests
  that put a workspace root in a temporary directory sit on tmpfs, so the list of file systems
  must be decided from a tmpfs probe as well.
