---
format: aep.planning-md/3
id: review-result:replay-grant-security-1
kind: review-result
status: active
title: Security review of the grant comparison in the deciding transaction, first pass
relations:
- reviews: story:replay-compares-the-grant-in-the-transaction-that-decides
revision: 1
---
# Security review of the grant comparison in the deciding transaction, first pass

Reviewed: branch `story/replay-grant` at `0c6fbb80f`, 7 files, by an independent reviewer on
2026-11-23. Verdict: needs change in public text only; the reviewer found no rule the code fails
to hold.

## What was run

- Store library: 128 cases before, 131 with the reviewer's three; all passed in 27.2 s.
- Daemon library: 130 before, 138 with the reviewer's eight; all passed in 2.3 s.
- Clippy on both crates with `-D warnings`: exit 0.
- Three store mutants on a scratch copy: two entry points without the comparison (red, the
  story's cases name those two of twelve); hash before grant (red, twelve of twelve); a
  comparison without case that treats the empty grant as none (the story's seven cases green,
  the reviewer's table red).

## What was read and found correct

- All 19 mutation routes that carry an operation identity reach a store entry point whose first
  read is the shared comparison. The record carried by a pending answer is discarded at both
  daemon sites. No statement updates a recorded grant and no operation row is deleted.
- A request answered from a row writes nothing: 4,320 store calls and 8 daemon requests left the
  change counter and the event position where they were.
- One entry point is a single `SELECT`; the other eleven are immediate transactions.
- Removing the daemon's separate grant read lost no check.

## Findings

Five, in the record's findings: two sentences of public text wider than the code (closed in the
second commit), two gaps in the story's own cases (closed by keeping the reviewer's cases), and
one answer that exists on `main` and is now stated in ADR 0017's addendum.

## Not checked

The daemon's integration tests, the runtime vectors, the gate and the delegated lane; requests
on real parallel tasks; a second process on one store file; the hosted TLS listener. The two
daemon mutants were prepared and not run.

```findings
[
{"file":"CHANGELOG.md","line":23,"category":"contract-drift","severity":"warning","verdict":"NEEDS-CHANGE","origin":"introduced","message":"The entry and the doc comment of LoadedOperation::answer_to said every read of a row compares the grant, while GET /v1/ops/{op} and the operation events return a row without a comparison, as design 09 section 4 decides. Closed: both sentences say mutation request, and the entry names the read routes as unchanged."},
{"file":"CHANGELOG.md","line":28,"category":"contract-drift","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"'A refusal decided for a request is never replaced' was wider than the addendum and the code, where a refusal decided after decoding yields to the row. Closed: the entry uses the addendum's words."},
{"file":"crates/substrate-store/src/operations/replay_grant_tests.rs","line":23,"category":"mutant","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"A comparison without case that treats the empty grant as none left the story's seven store cases green. Closed: the reviewer's table of 4,320 requests fails on it and is kept."},
{"file":"crates/substrate-daemon/src/app/tests.rs","line":797,"category":"mutant","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"No case of the story drove a route's accepting transaction or a refusal before dispatch with a row under another grant. Closed: the reviewer's eight daemon cases drive both and are kept. The two daemon mutants were prepared and not run."},
{"file":"crates/substrate-daemon/src/app/operations.rs","line":161,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"pre-existing","message":"A refusal row recorded without a grant for a missing or invalid context replays to a later request of the same operation identity and input that presents a valid context. Unchanged; ADR 0017's addendum now states it."}
]
```
