---
format: aep.planning-md/3
id: approval-record:runbook-answers-the-questions-of-adr-0050-2026-11-22
kind: approval-record
status: active
title: Stakeholder (session runbook) answers the questions of the change record's ADR (2026-11-22)
tags:
- sdc
relations:
- decides: story:exec-file-changes-are-recorded
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-11-22T01:11:07Z", actor: "agent:runbook", revision: 2, executor: "agent:substrate"}
---
# Stakeholder (session runbook) answers the questions of ADR 0050, the per-run change record

## Who decided

Session `runbook`, the session formerly named `sdc` (agent:sdc), not a person, on 2026-11-22, as
the stakeholder of `story:exec-file-changes-are-recorded` and the consumer whose own recorder the
record replaces, under `approval-record:stakeholder-decides-for-the-operator-2026-11-21`.

## What was asked

Eighteen questions of the ADR's draft, each with a default named first, after the draft was
revised around a kept manifest. Question 12, whether the confinement floor refuses the ioctl
`EXT4_IOC_MOVE_EXT`, was put to the operator session and is not decided here.

## Its words

> ADR 0050 answers. Defaults taken on 1, 2, 3, 5, 6, 7, 8, 9, 10, 11, 13, 14, 15, 16, 17, 18;
> one deviation (4); Q12 is the operator's, my reading as consumer: A.
> 4 = B, the profile may include `.git`. Reason: in my runbooks the confined run never commits;
> a host-side tool commits after it. A run that writes `.git/hooks/*`, `.git/config` or
> `.git/info/attributes` plants code or a filter that the host tool then executes outside the
> confinement. I need those changes in the record, so my profile will set it. If a profile
> member costs more than a fixed rule, this serves me equally: `.git` left out except `hooks/`,
> `config` and `info/`, which are always covered. Choose the cheaper one and tell me which.
> Conditions on the defaults (say if one costs you a story):
> 1 A: the unavailable record carries the count of changed paths, so the embedder can tell
> 10 001 from 2 000 000.
> 3 A: the record names the excludes that were applied, and a directory with an excluded name
> is itself listed when it is created or deleted (per 5 A) though its contents are not; a new
> `target/` at a new place is then visible. My profile: `target`, `node_modules`.
> 6 A: I rewrite my reader to one `rename` entry with `replaced`; `sdc.fs-change/1` is mine and
> changes with it.
> 7 A and 8 A: the unavailable record names the second writer (route and path for 7; pid and
> executable for 8), so the embedder can remove the cause. My bound programs do not run beside
> an agent run in the same workspace; a violation of that is my defect and I want to see it.
> 9 A: fine because Loom copies the record into its own store at run end; state in the ADR that
> the embedder must fetch within the hour.
> 13 A: `prepare_file_changes` needs a way to learn that the manifest is ready (a status call or
> a bounded wait), so Loom starts the first run after it and never sees `manifest-not-ready` in
> the normal path.
> 16 A: my workspaces are on ext4 (`runs/` and the session scratch); none on tmpfs or overlay.
> 18 A as a stated limit, given 12 A closes the one known way.
> Bounds as proposed are fine for my trees: with `target` excluded my checkout is far below
> 200 000 entries, and a case workspace is a few thousand.

## The questions and the options it took

1. More than 10 000 changed paths: the record is unavailable, and carries the count.
2. Binary files: digests only.
3. `exclude`: entry names at any depth; the record names the excludes applied, and a directory
   with an excluded name is listed when it is created or deleted.
4. `.git`: the stakeholder's deviation, see its words; the ADR picks the cheaper of the two forms
   it offers and says which.
5. Directories and special files: listed for create and delete only.
6. A rename onto an existing path: one `rename` entry with `replaced`.
7. A write through Substrate's file routes during a run: the record is unavailable and names the
   route and the path.
8. A host process with its working directory in the workspace: a second writer; the unavailable
   record names its pid and executable.
9. Retention: 1 h and 128 records; the embedder fetches within the hour.
10. A change time beside the modification time: not in the record.
11. One driver per workspace root: assumed.
13. The first record in a workspace: a background first walk, `manifest-not-ready` until it is
    done, `prepare_file_changes` with a way to learn that the manifest is ready.
14. Start wait: 5 s for every run.
15. Tree bounds: 200 000 entries and 8 GiB.
16. Overlay and tmpfs: not trusted; every walk reads every file there.
17. Store: one copy of each text file up to 1 MiB per kept tree, 8 GiB ceiling.
18. A kept digest made wrong by a process outside Substrate: a stated limit.
