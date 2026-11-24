---
format: aep.planning-md/3
id: review-result:result-widths-adversary
kind: review-result
status: active
title: 'Adversary pass: the widths of the results the ledger records'
relations:
- reviews: story:every-success-result-fits-the-ledger-before-its-side-effect
revision: 1
---
# Adversary pass: the widths of the results the ledger records

2026-11-24, on the first commit of the branch `feat/result-widths-in-the-wire-crate`
(`6afd8466b`, base `c407457cc`). The adversary added seven cases in
`crates/substrate-wire/src/adversary_result_width_tests.rs` and did not edit the
implementation. Verdict: not broken. S1: 0. S2: 2. S3: 4.

## Held

- A member counted in one-byte characters admits no costlier character: the identifiers, the
  name of a digest, a label key and a root name are held to ASCII by their own validators.
- No integer a stored result holds is written in more than twenty characters.
- A vocabulary a width names by one variant has no other variant.
- The list is the widest form of an applied network.
- An exec width counts each entry past a list bound as each one within it.
- An instant is written in at most thirty characters through the year 9999, under the 35 the
  widths count.
- Every stored process result is made without `cpu`, `memory` and `listening`, and a lease's
  `keep_reason` is set at the start only.

## Findings and what was done

1. S2. Rule asked: a bound root's `host_path` is at most the 4,000 bytes its width counts. No
   code holds a declared path or a Git checkout path to that length today. Nothing calls the
   width yet, so no answer is wrong. Owed by the unit that turns the refusal on; the story
   names the three places. The adversary's case
   `limit_a_bound_root_is_counted_for_a_host_path_of_at_most_the_published_bytes` states the
   limit.
2. S2. Rule asked: a refusal's `code` is counted at a published bound. The width counts 128
   characters, bundle `0.27.0` publishes no length, and ADR 0053 had no row for it. Done: the
   ADR's decision 7 has the row and decision 11 the `maxLength`. The check at the daemon's
   driver port is owed by the same later unit.
3. S3. The session width did not say what it assumes. Done: its documentation and the ADR's
   table state that `workspace` and `capability_snapshot` are counted as a minted identifier
   and the name of a digest, which holds after the start is validated and its workspace
   admitted.
4. S3. An exec width for a list past its bound. Held by two adversary cases.
5. S3. The width of a start with a long root list cost time that grew with the list (80,000
   roots: 1.02 s). Done: a root past the bound is counted by arithmetic, not by a built value.
6. S3. The lease bound was compared with one of the copies the bundle publishes. Done:
   `every_lease_bundle_0_27_0_publishes_states_the_actor_and_principal_bounds` reads all 13.

The ADR's figures were corrected to the ones the tests hold (110,926, 3,926, and 127, 125 and
122 bytes), and its rule for an integer and an ASCII string was written as the code counts.

## Not established

What the case `every_member_of_a_result_type_is_present_in_its_widest_form` misses is listed
in the story. No daemon path was run: nothing calls the widths yet.
