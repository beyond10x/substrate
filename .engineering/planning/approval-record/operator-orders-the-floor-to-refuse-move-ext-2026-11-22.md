---
format: aep.planning-md/3
id: approval-record:operator-orders-the-floor-to-refuse-move-ext-2026-11-22
kind: approval-record
status: active
title: Operator orders the floor to refuse EXT4_IOC_MOVE_EXT (2026-11-22)
tags:
- host-operator
relations:
- decides: story:floor-refuses-the-ioctl-that-exchanges-file-blocks
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-11-22T01:11:04Z", actor: "human:timo", revision: 2, executor: "agent:substrate"}
---
# Operator orders the floor to refuse `EXT4_IOC_MOVE_EXT` (2026-11-22)

## Who decided

The operator, on 2026-11-22. His answer reached this repository's session twice, each relayed by
another session; neither was typed in this session.

## What was asked

Put to session `host-operator` as a decision without a default, because it changes the isolation
floor for every confined run: if `EXT4_IOC_MOVE_EXT` also works inside a confined run, A the
floor refuses the request for every confined run and session, as a story of its own ahead of the
change record; B only for runs that ask for a change record; C the floor stays and the limit is
stated. The same question was number 12 of the stakeholder's questions on the change record's
ADR.

## The words, as relayed

Session `host-operator`:

> host-operator relays Timo's pick, about 01:05 CET, to "If EXT4_IOC_MOVE_EXT also works inside
> a confined run, what should the isolation floor do?": "A: refuse for all runs". If your in-run
> verification shows it already refused, tell me and the story is void; otherwise its own story
> ahead of the change record, design note first, the before/after case.

Session `runbook`:

> Timo typed his answer to your Q12 in my session at about 2026-11-22 00:57Z: "substrate -> A".
> A = the confinement floor refuses `EXT4_IOC_MOVE_EXT` for every confined run. It is his word,
> relayed verbatim.

## The condition, and its result

The order held only if the call succeeds inside a real confined run. It does: one run on the
`linux` adapter with a writable workspace on `ext4` returned 0 from the call, and the donor
file's content on disk changed while its device, inode, size, mtime and ctime did not. The lines
are in the body of `story:floor-refuses-the-ioctl-that-exchanges-file-blocks`.

## What follows

- `story:floor-refuses-the-ioctl-that-exchanges-file-blocks` moves draft, proposed, active, and
  is worked ahead of `story:exec-file-changes-are-recorded`: design note, failing case, the
  seccomp change, two reviews, then the pull request to the operator session.
- The change record's ADR keeps the refusal as its stated precondition.
