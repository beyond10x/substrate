---
format: aep.planning-md/3
id: approval-record:operator-picks-for-the-deny-list-the-floor-and-release-0-24-0-2026-11-22
kind: approval-record
status: draft
title: Operator's picks for the deny-list, the floor and the next release (2026-11-22)
relations:
- decides: story:executable-deny-list
- decides: story:floor-refuses-the-ioctl-that-exchanges-file-blocks
revision: 1
---
# Operator's picks for the deny-list, the floor and the next release (2026-11-22)

## Who decided

The operator, on 2026-11-22. Each pick reached this repository's session as a relay by session
`host-operator`; none was typed in this session, and none is a decision of that session.

## The words, as relayed

> host-operator relays Timo's pick, 2026-11-22 05:48 CET, to "substrate PR 243 (executable
> deny-list) … Merge?": "A: merge". Merge with the bot, then take main into 254 and 255. Back to
> me: the merge sha.

> host-operator relays Timo's picks, 2026-11-22 06:59 CET. D15 "when is the next substrate
> release cut?": "A: 0.24.0 after 254-256" — one release with the deny-list and the three
> security fixes, each named under Security; loom pins a commit until then. Come back with the
> release PR. PR 254: "A: merge, keep f2fs". Merge with the bot; ADR 0050 keeps saying who added
> the f2fs row.

## What follows

- Pull request 243 was merged by the bot as `b280c4ca7706338e869e5ec43903aecd1ce7bf0d`;
  `story:executable-deny-list` moves to `implemented`. The stakeholder session `runbook` holds its
  acceptance open until a confined Runbook step has run on a Substrate that contains the merge.
- Pull request 254 was merged by the bot as `79e0260f680bda8a677b1780032afdae4a78af73`;
  `story:floor-refuses-the-ioctl-that-exchanges-file-blocks` moves to `implemented`. The row for
  `F2FS_IOC_SEC_TRIM_FILE` was added by this repository's session under the operator's rule and
  is kept by his pick.
- The next release is 0.24.0, cut after pull requests 255 and 256 are merged. Its
  release-prepare pull request goes to the operator session.
