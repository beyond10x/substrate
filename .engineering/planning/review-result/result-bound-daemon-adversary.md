---
format: aep.planning-md/3
id: review-result:result-bound-daemon-adversary
kind: review-result
status: active
title: 'Adversary pass: the daemon refuses a start whose result cannot be recorded'
relations:
- reviews: story:every-success-result-fits-the-ledger-before-its-side-effect
revision: 1
---
# Adversary pass: the daemon refuses a start whose result cannot be recorded

2026-11-24, on the second commit of the branch `feat/result-bound-refusal` (`e592fcee9`, parent
`be4fe83fc`): ADR 0053, unit 2, the daemon's part. The adversary added three cases and did not
edit the implementation: one in `crates/substrate-store/src/result_bound_adversary_cases.rs` and
two in `crates/substrate-daemon/tests/processes/result_bound_adversary.rs`. Verdict: one rule
broken, in a store configuration only tests open. Cases executed 608 to 611, red 1. S1: 0.
S2: 1. S3: 2.

## Held

- Every member of an exec, a process record, a session and a v1 file answer is written as sent
  or counted at its bound, from the place each driver builds it. A v1 path wide enough does not
  pass the 65,534-byte limit of the request's address.
- No lexical or capability check runs after the width decision at any of the five starts.
- `within_result_reserve` has five callers, the five decided kinds.
- Every terminal update that stores a success goes through `terminal_success`, and every
  success answer through `completed` or `replay`.
- The answer and the stored row are judged on one value, and a replay reads the stored row.
- The v2 file routes keep `fit_json_bytes`.
- The reserve is one constant, and the width is decided before admission: a workspace of
  another subject gets the answer of an absent one.
- Two cases, green, hold what no case held before: an operation id refused for the result bound
  answers a different request `operation.request-conflict`, and so does a completed one, each
  against the shipped reserve of 524,217 bytes.

Not established by the pass: the v1 file answers of the namespace driver are decoded by shape
only, so their width rests on the executor echoing `workspace` and `path`. No mutant was run.

## Findings and what was done

1. S2. Rule asked (decisions 1 and 8): a row always ends, so a completion over the reserve ends
   its row with `operation.result-unrecorded`. A store opened with a terminal headroom under
   the bytes that outcome needs left the row accepted. Only `Store::open_with_config` sets a
   headroom, and only tests call it; the daemon opens the default of 524,288 bytes. Case:
   `every_headroom_the_store_opens_with_ends_an_over_reserve_completion_terminal`. Done in
   `7a7314e39`: a store does not open under `StoreConfig::minimum_terminal_headroom_bytes`
   (35 + 8 + 379 = 422 bytes), refused `OperationTerminalHeadroomBelowUnrecorded` with both
   figures; one case at the figure and one a byte under.
2. S3. ADR 0053 decision 7 placed the bounds of the usage `code` and `message` and the refusal
   `code` and `message` at the daemon's driver port. They are held at completion, by
   `members_within_bounds`. No driver emits a value over a bound. Done in `7a7314e39`: the two
   rows of decision 7 say "at completion"; no check was added at the driver port.
3. S3. The `MAX_PROCESS_CWD_BYTES` guard answers as the kernel does for a path of 4,096 bytes
   or more, so no case holds that it runs before any file-system call. Not done: `plan_start`
   has no seam to count a call. The commit message of `7a7314e39` names it as not held.

Runs of the pass, release profile, in the build pool: the library cases of the wire crate, the
daemon, the SDK and the store 470 run, 469 passed, 1 failed (the case of finding 1); the
daemon's `driver_port`, `http` and `processes` 141 passed, 1 skipped.
