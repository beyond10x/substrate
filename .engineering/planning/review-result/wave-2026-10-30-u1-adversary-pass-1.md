---
format: aep.planning-md/3
id: review-result:wave-2026-10-30-u1-adversary-pass-1
kind: review-result
status: active
title: Adversary pass 1, unit u1 (the clean-room conformance runner)
relations:
- reviews: story:remote-clean-room-conformance
revision: 1
---
# Adversary pass 1, unit u1 (the clean-room conformance runner)

Agent type `aep:adversary`. Tree on integration head `81b9e99b` (unit commits `3af50843`,
`855d7730`). Six cases committed in
`crates/b10x-substrate-conformance/tests/adversary_report_and_input.rs` (`eff9ecdf`, merged as
`4a338a06`), run against an in-process TLS stub: five red, one green control. `local_endpoint`'s
portable scenario is also red under the wave's `build-slot.sh` (an inherited lock descriptor).
`cargo clippy -p b10x-substrate-conformance --all-targets -- -D warnings` fails. Verdict: red.

```findings
- file: crates/b10x-substrate-conformance/src/cases_exec.rs
  line: 468
  category: lint
  severity: blocker
  verdict: NEEDS-CHANGE
  origin: introduced
  message: restart() is 104 lines under workspace clippy pedantic, so the gate's clippy step fails on the unit as committed.
- file: crates/b10x-substrate-conformance/src/token.rs
  line: 185
  category: security
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: The runner marks only the descriptors it opens close-on-exec, so an inheritable descriptor from its parent reaches the token command and the restart hook, against design 31 sections 4 and 5.
- file: crates/b10x-substrate-conformance/src/lib.rs
  line: 281
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: snapshot_read copies the server's snapshot string into the report unchecked, so a non-conforming endpoint puts free text into the closed report and the report fails its own schema.
- file: crates/b10x-substrate-conformance/src/lib.rs
  line: 476
  category: correctness
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: An --out that cannot be created is found only after the whole inventory, mutations included, has run against the endpoint, and exits 3 instead of 2.
- file: crates/b10x-substrate-conformance/src/cases.rs
  line: 780
  category: contract-drift
  severity: note
  verdict: NEEDS-CHANGE
  origin: introduced
  message: unserved_answer records any three-digit status, and the schema bounds it to 100 to 599, so a status of 999 leaves the report outside its schema.
- file: crates/b10x-substrate-conformance/src/lib.rs
  line: 136
  category: correctness
  severity: note
  verdict: NEEDS-CHANGE
  origin: introduced
  message: A --token-command that does not exist passes the input check and exits 3 token-unavailable, not 2.
- file: crates/b10x-substrate-conformance/src/lib.rs
  line: 301
  category: correctness
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: A machine body the wire types refuse stops the run with exit 3 and no report, instead of failing the core case machine.envelope-decodes with exit 1.
- file: xtask/src/chart_cluster.rs
  line: 3146
  category: judgement
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: The lanes trust the runner's verdict without re-checking core groups, the digest or the schema; this matters only for a report inconsistent with itself, which the runner does not produce.
- file: xtask/src/namespace_lane_run.rs
  line: 1180
  category: correctness
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: Step 4 restarts any daemon that exits and only reports the count, so a crash the hook did not cause is masked.
- file: crates/b10x-substrate-conformance/src/cases_exec.rs
  line: 283
  category: judgement
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: exec.no-egress asserts an empty route table where design 31 says an outbound connect fails; no driver reachable today has exec facts plus routes.
- file: crates/b10x-substrate-conformance/src/report.rs
  line: 193
  category: test-gap
  severity: note
  verdict: CONFIRMED
  origin: introduced
  message: Twelve pinned registry operations have no case and are not in not_covered, so a report does not show what it did not exercise.
```

Held: an absent or skipped core case never gives `passed`; `--only` never passes; every operation
id a case names is in the pinned registry; an undeclared scope yields `incomplete`; case order is
fixed and the `.sha256` is the digest of the exact bytes written; a bad URL, unreadable trust
roots, an invalid token fd and an existing `--out` exit 2.

Coordinator decision: findings 1 to 7, 9 and 11 go to the fix unit on
`impl/remote-clean-room-conformance-fix`; the two infeasible ones stay as they are.
