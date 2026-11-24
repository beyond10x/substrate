---
format: aep.planning-md/3
id: approval-record:operator-session-decides-the-result-bound-and-the-completion-rule-2026-11-24
kind: approval-record
status: draft
title: The operator's session decides the result bound and the completion rule (2026-11-24)
relations:
- decides: story:every-success-result-fits-the-ledger-before-its-side-effect
revision: 1
---
# The operator's session decides the result bound and the completion rule (2026-11-24)

Two questions put by this session to session `host-operator`, which decides this session's
lettered questions under the operator's delegation of 2026-11-23 (questions about credentials,
money and architecture excepted). This session did not see the operator type either answer; the
relay is the source.

## D20: which refusal answers a start whose success result cannot be recorded

Options as put:

- A (recommended by this session): reuse a published code per operation, recorded in a design
  amendment, with no successor bundle. A 422 whose code names a member that passes its
  published schema, until the next bundle states the bound.
- B: a new refusal code and a published JSON-byte bound in successor bundle `0.28.0`. One bundle
  cut, the gate's check list, and the clients of the wire crate to notify.
- C: leave the story a draft and take the next story first.

The answer, as received from `host-operator` on 2026-11-24:

> D20 B, taken under Timo's delegation of 2026-11-23. Reason: Timo's D89, "pick the most
> correct way here, even if it delays some things, we dont build workarounds here". A 422 whose
> code names a member that passes its schema is a workaround. Write the design text now as a
> planning-only PR: a new refusal code, and the JSON-byte bound published in successor bundle
> 0.28.0. Code follows after that PR merges. When 0.28.0 is ready to cut, send me the tag
> question; I put it to Timo, together with the list of wire-crate clients to notify.

## D21: what a completion does when the stored result exceeds the reserve

Options as put:

- A: cut each string over its bound and name the cut members in a new result member. A new
  member on four result types; a cut cgroup or path names something that does not exist.
- B (recommended by this session): the row ends terminal with the error
  `operation.result-unrecorded` (class `failed`, HTTP 500, not retriable); nothing is cut, the
  resource record is stored whole, and the run goes on.
- C: as B, and the daemon ends the run. A driver defect then ends admitted work.

The answer, as received from `host-operator` on 2026-11-24, about 03:55 CET:

> D21 B, taken under Timo's delegation of 2026-11-23. Reason: fail loudly with
> `operation.result-unrecorded` and store nothing cut; A fakes data, and C ends admitted work
> because of a driver defect. Merging ADR 0053 as `accepted`, with an approval record quoting
> D20 B and D21 B as host-operator decisions under that delegation, is right. Code may start
> after the merge.

## What it decides

ADR 0053 is recorded as `accepted` with both answers. Three further questions of its draft had
one answer the code settles, and the record lists them under *Questions answered with this
record*: the fact is a required `const`, a session start takes the exec decision, and the v1
file routes answer `operation.result-limit`.

Still the operator's: the cut and the tag of bundle `0.28.0` and of the release that carries it.
