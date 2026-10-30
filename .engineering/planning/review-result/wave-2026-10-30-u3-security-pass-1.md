---
format: aep.planning-md/3
id: review-result:wave-2026-10-30-u3-security-pass-1
kind: review-result
status: active
title: Security review pass 1, unit u3 (a 409 on a preconditioned Pod delete)
relations:
- reviews: story:preconditioned-delete-409-is-not-a-uid-mismatch
revision: 1
---
# Security review pass 1, unit u3 (a 409 on a preconditioned Pod delete)

Agent type `aep:security-reviewer`. Tree on integration head `e83b69863` (unit commit `54fcd40ea`).
Two cases committed in `crates/substrate-kubernetes-client/src/client_tests.rs` (`f02ac8743`, merged
as `ffd2e6c1`), both green: a follow-up read that is not answered after a 409 never ends the removal
as gone; the delete after a 409 carries the read uid and spares a late replacement.
`cargo test -p b10x-substrate-kubernetes-client --lib`: 41 passed; `cargo test -p
b10x-substrate-kubernetes --lib`: 91 passed. Verdict: green, no blocker.

```findings
- file: crates/substrate-kubernetes-client/src/api.rs
  line: 641
  category: contract-drift
  severity: warning
  verdict: NEEDS-CHANGE
  origin: introduced
  message: The story's acceptance says the removal parses the 409 Status reason and details before the GET; remove_once lets the GET alone decide, which is safe but disagrees with the acceptance text.
- file: crates/substrate-kubernetes-client/src/api.rs
  line: 605
  category: robustness
  severity: note
  verdict: CONFIRMED
  origin: pre-existing
  message: remove_until_gone_seen is unbounded by documented design, so a webhook that keeps denying DELETE with 409 keeps the pod.rs:1565 task retrying after the run is already reported Unknown, instead of ending early as gone.
- file: crates/substrate-kubernetes-client/src/api.rs
  line: 644
  category: robustness
  severity: note
  verdict: CONFIRMED
  origin: pre-existing
  message: Before any read has given the trace a uid, remove_once deletes by name without a precondition (abandon after a lost create answer), as design 28 states; the uid rule holds only once a uid is known.
```

Held: every retry after a 409 takes its uid from `seen.uid()`, which never changes once set; a GET
error after a 409 is `false` in both callers; the bounded round makes exactly `remove_attempts`
tries and returns `false`.
