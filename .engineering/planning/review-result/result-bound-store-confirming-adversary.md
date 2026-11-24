---
format: aep.planning-md/3
id: review-result:result-bound-store-confirming-adversary
kind: review-result
status: active
title: 'Confirming adversary pass: an unrecorded completion''s events and the least headroom'
relations:
- reviews: story:every-success-result-fits-the-ledger-before-its-side-effect
revision: 1
---
# Confirming adversary pass: an unrecorded completion's events and the least headroom

2026-11-24, on the third commit of the branch `feat/result-bound-refusal` (`7a7314e39`): ADR
0053, unit 2, the store's part of the daemon change. The adversary added fifteen cases in
`crates/substrate-store/src/result_bound_completion_path_cases.rs` and did not edit the
implementation. Verdict: one rule of the commit broken, in a store configuration only tests
open; one finding outside the commit. Library cases executed 476 to 491, red 2. S1: 0. S2: 2.

## Held

- The events of a recorded completion are those of the commit before, path by path: the
  general completion (success and the caller's error), the exec completion, the exec retire,
  the session start, lease renewal and observation, the workspace adopt and destroy, and the
  exec and workspace lease renewals, each by a case that holds the literal list. The process
  renewal and the process stop were held by reading; the next commit adds a case for each.
- An unrecorded completion writes `operation.failed` first, then the resource's own events as
  the recorded completion writes them, no event twice: exec retire, workspace adopt and destroy,
  exec lease renewal, sessions.
- Every call of `append_completion_events` runs inside the caller's transaction before the
  commit, and every early return drops the transaction.
- A replay answers the stored error with status 500 and writes no event.

Not established by the pass: no case was run on the commit before; no mutant was run.

## Findings and what was done

1. S2. Rule asked (decisions 1 and 8): every row the store accepts can end with
   `operation.result-unrecorded` at the least headroom a store opens with. The least headroom
   did not count the `resource` column a terminal update writes, and the store did not hold an
   operation id to the length the error's width assumes. The daemon names the resource at every
   acceptance, validates the id at the wire and opens the default headroom of 524,288 bytes.
   Case: `at_the_least_headroom_an_over_reserve_completion_that_names_its_resource_ends_its_row`.
   Done in the branch's next commit: the least headroom is 35 + 8 + 379 + 32 = 454 bytes,
   computed from its parts; the store accepts an operation only with an id of at most
   `MAX_OPERATION_ID_CHARS` characters of the wire's operation-id alphabet and a resource of at
   most `MAX_RESOURCE_ID_CHARS`; a terminal update that names a longer resource rolls back and
   the row stays accepted, which ADR 0053 decision 8 states.
2. S2, outside the commit. Rule asked: the first event of a session operation is
   `operation.accepted`. The store's documented projection for sessions
   (`published_transition`) publishes it otherwise, and has done so since before this branch.
   Not done here: it changes the published event stream of every session operation and owes a
   design answer first. Filed as
   `story:a-session-operations-acceptance-is-published-as-accepted`, which carries the rule and
   the case's name. The case was taken out of the file; the doc comment of
   `published_transition` now states all three session mappings.

Three of the pass's cases were changed with finding 1: a resource id over
`MAX_RESOURCE_ID_CHARS` is now refused at acceptance, so the workspace adopt case reaches the
reserve with two labels at their bound, the session chain uses an id of 32 characters, and the
exec retire and workspace destroy cases hold that their widest result is recorded.

Runs of the pass, release profile, in the build pool: the library cases of the wire crate, the
daemon, the SDK and the store 491 run, 489 passed, 2 failed (the two findings); the daemon's
`driver_port`, `http` and `processes` 144 passed, 1 skipped.
