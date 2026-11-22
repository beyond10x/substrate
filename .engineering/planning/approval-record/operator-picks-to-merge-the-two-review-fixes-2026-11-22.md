---
format: aep.planning-md/3
id: approval-record:operator-picks-to-merge-the-two-review-fixes-2026-11-22
kind: approval-record
status: draft
title: Operator's picks to merge the two review fixes (2026-11-22)
relations:
- decides: story:git-changes-reader-reads-only-the-admitted-workspace
- decides: story:read-only-mount-sources-refuse-the-daemons-private-paths
revision: 1
---
# Operator's picks to merge the two review fixes (2026-11-22)

## Who decided

The operator, on 2026-11-22. Each pick reached this repository's session as a relay by session
`host-operator`; neither was typed in this session, and neither is a decision of that session.

## The words, as relayed

> host-operator relays Timo's pick, 2026-11-22 10:13 CET, to "substrate PR 256 (refuse read-only
> sources naming private paths). Merge?": "A: merge with the bot". Merge head a5d4f65f2 only if
> it is still that head with checks green. PR 255 goes to him now as its own question.

> host-operator relays Timo's pick, 2026-11-22 10:15 CET, to "substrate PR 255 (Git changes
> reader reads only the admitted workspace). Merge?": "A: merge with the bot". He was shown head
> 1de70cd8a; head ec695c457 (that plus a merge of main, CHANGELOG conflict only) is covered:
> merge it once every hosted check is green on it. Then the 0.24.0 release PR (his pick of
> 06:59: "A: 0.24.0 after 254-256").

## What follows

- Pull request 256 was merged by the bot at `a5d4f65f2`, 21 of 21 checks green, as
  `cd2d9db108319e872992ae1b347c552bd25d2634`;
  `story:read-only-mount-sources-refuse-the-daemons-private-paths` moves to `implemented`.
- Pull request 255 was merged by the bot at `ec695c457`, 20 of 20 checks green, as
  `6b7aa0a9fd8b14e58f8e616c16a50d1151474150`;
  `story:git-changes-reader-reads-only-the-admitted-workspace` moves to `implemented`.
- The release-prepare pull request for 0.24.0 carries these records.
