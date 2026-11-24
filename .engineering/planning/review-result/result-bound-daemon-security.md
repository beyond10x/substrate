---
format: aep.planning-md/3
id: review-result:result-bound-daemon-security
kind: review-result
status: active
title: 'Security review: the daemon refuses a start whose result cannot be recorded'
relations:
- reviews: story:every-success-result-fits-the-ledger-before-its-side-effect
revision: 1
---
# Security review: the daemon refuses a start whose result cannot be recorded

2026-11-24, on the second commit of the branch `feat/result-bound-refusal` (`e592fcee9`, parent
`be4fe83fc`): ADR 0053, unit 2, the daemon's part. The reviewer added three cases in
`crates/substrate-daemon/tests/processes/result_bound_security_review.rs` and did not edit the
implementation. Verdict: no rule broken. Cases executed 139 to 142 (`driver_port`, `http`,
`processes`), red 0. S1: 0. S2: 1. S3: 2.

## Held

- Each of the five decisions comes before the workspace lock, admission, the lease and the
  reservation. Before them runs only read-only path resolution and the `cwd` stat. Replay and
  conflict go through `record_bound_refusal`.
- No member is missing from a width: `result_widths.rs` was compared with every place a record
  is built, in the host driver, the namespace driver and the daemon's `wire_record`. No stored
  process record carries `cpu`, `memory` or `listening`.
- Only `lease.clock-unavailable` comes after the width, where decision 3 puts the lease.
- `within_result_reserve` is called at the five starts only. A process stop is begun also where
  its row ends unrecorded.
- Every success write in the store goes through `terminal_success`, and every success answer to
  a mutation through `completed` or `replay`, on the value the store wrote. Recovery selects
  rows by state.
- The lengths of a writable root and a workspace root are checked on the bytes the driver
  joins. An aperture is IPv4. The namespace driver's cgroup name has a fixed width.
- `FileMutationResult` has no member `members_within_bounds` reads.
- The scope of both codes is one subject, identifiers are bounded, both messages are fixed
  sentences, and `/metrics` counts both codes under a closed label set.
- Three cases, green, against the shipped reserve of 524,217 bytes: a start over the reserve is
  refused `operation.result-limit`, nothing starts, and its operation id stays bound to that
  request; a start that another code refuses keeps that code; a start at the reserve completes
  its start, its renewal, its stop and its collected end.

Not established by the review: no mutant was run. The exec, session and v1 file decisions were
not exercised at the shipped reserve. A daemon restart between the store's write and the answer
was not exercised.

## Findings and what was done

1. S2. Rule asked (decision 8): an unrecorded completion writes the same events on every path.
   The four completion paths wrote four forms. Done in `7a7314e39`: one rule, built by
   `append_completion_events` in `crates/substrate-store/src/result_bound.rs`: the operation's
   `operation.failed`, carrying the error, and the resource's own events as the recorded
   completion of the same path writes them. Four cases, one per path, in
   `crates/substrate-store/src/result_bound_event_cases.rs`. A session has no event of its own:
   its unrecorded completion writes `operation.failed` alone. ADR 0053 decision 8 states the
   rule.
2. S3. Decision 7's placement of the usage and refusal `code` and `message` bounds. The
   adversary pass of the same commit has the finding; done there.
3. S3. No case holds that the `MAX_PROCESS_CWD_BYTES` guard runs before a file-system call. The
   adversary pass of the same commit has the finding; not done, and named in the commit
   message of `7a7314e39`.

Runs of the review, release profile, in the build pool: the daemon's `driver_port`, `http` and
`processes` 142 passed, 1 skipped; the library cases of the wire crate, the daemon, the SDK and
the store 469 of 469.
