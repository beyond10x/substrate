---
format: aep.planning-md/3
id: review-result:result-bound-store-confirming-adversary-2
kind: review-result
status: active
title: 'Second confirming adversary pass: the store''s bound on an operation row'
relations:
- reviews: story:every-success-result-fits-the-ledger-before-its-side-effect
revision: 1
---
# Second confirming adversary pass: the store's bound on an operation row

2026-11-24, on the commit `7606bf6f3` of the branch `feat/result-bound-refusal` (ADR 0053,
the store). The adversary added six cases in a test file of its own and did not edit the
implementation. Verdict: four cases red, all against the store directly; none was reached
through the shipped daemon, whose targets stayed at 174 passed, 1 skipped. Library cases of
the wire crate, the daemon, the SDK and the store executed 494 to 500, red 4. S1: 0. S2: 1.
S3: 4.

Nothing of this branch is released.

## Held

- Every terminal `UPDATE operations` writes only `state`, `terminal_at`, `resource`,
  `outcome_json` and `response_status`; `accepted` to `terminal` changes no byte of the state.
- The operation-id alphabet has no character JSON escapes and no multi-byte character, so the
  widest completion error stays at 379 bytes.
- The one insert that writes `accepted` runs the bound check first; recovery, migration and
  replay insert nothing.
- Every acceptance in the daemon passes a resource of at most 31 ASCII characters, or looks
  the resource up before it accepts.
- `SCHEMA_VERSION` and the fingerprint fields do not move; a replay runs before the check.
- A rolled-back terminal update writes no event, usage, workspace or lease (green case).
- The operation-id alphabet and lengths did not move (green case).

## Findings and what was done

1. S2. Rule asked: the resource bound and the headroom count the same unit. Case
   `adv4_a_resource_of_max_chars_in_two_byte_characters_named_at_completion_ends_the_row`.
   Done: the bound is in bytes, `OperationResourceTooLong { bytes, max }`.
2. S3. Rule asked: a row whose accounting is rebuilt while it is `unknown` still ends within
   the least headroom. Done: a rebuild charges a row that is not terminal with the 8 bytes of
   `accepted`.
3. S3, kept. A refused row names the resource the caller named, at any length. ADR 0053
   decision 8 bounds the operations the store accepts; a refused row reserves no headroom and
   is charged its own bytes. The ADR and the doc comments now say so, and one case holds that a
   refused retire with a longer path identifier is recorded with it, reserves nothing and
   replays the same answer.
4. S3. Rule asked: a terminal update that names a resource over the bound rolls back wherever
   it runs. Done: the check is in `finalize_operation_accounting`, which each of the crate's 11
   terminal updates calls in its transaction; one case counts calls against terminal updates in
   every source file.
5. S3. The doc of `OperationIdInvalid` states the alphabet and the 128-character maximum; the
   wire's minimum of 16 is the wire's rule.

Decisions 1, 2 and 4 are of the session that works this repository, reported to the operator.
A first form of finding 3 (a refused row stored without a resource over the bound) was
withdrawn by the same session: the operation record of bundle `0.28.0` publishes no bound on
`resource`.

Nine cases in `crates/substrate-store/src/result_bound_row_accounting_cases.rs`, one changed in
`tests.rs`. Runs of the change, release profile, in the build pool: the library cases 502 of
502; the daemon's `driver_port`, `http`, `processes` and `contract_vectors` 174 passed,
1 skipped.
