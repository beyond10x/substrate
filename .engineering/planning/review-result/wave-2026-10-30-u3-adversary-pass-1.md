---
format: aep.planning-md/3
id: review-result:wave-2026-10-30-u3-adversary-pass-1
kind: review-result
status: active
title: Adversary pass 1, unit u3 (a 409 on a preconditioned Pod delete)
relations:
- reviews: story:preconditioned-delete-409-is-not-a-uid-mismatch
revision: 1
---
# Adversary pass 1, unit u3 (a 409 on a preconditioned Pod delete)

Agent type `aep:adversary`. Tree on integration head `e83b69863` (unit commit `54fcd40ea`). Five
cases committed in `crates/substrate-kubernetes-client/src/client_tests.rs` (`a1e8e938d`, merged as
`45a0e8b8`), all green: a read answered 503 or timing out after a 409 is never the Pod gone; a
terminating read of the same uid is present; another uid is gone after one delete and one read; an
unbounded removal against a persistent 409 never ends and deletes nothing.
`cargo test -p b10x-substrate-kubernetes-client --lib`: 44 passed. Verdict: green.

```findings
[]
```

Held: a 409 from a resourceVersion precondition is unreachable (the client sends only
`preconditions.uid`); the Pod adapter's test doubles never answer 409. The reviewer noted the
story's acceptance still asked for Status parsing; the coordinator amended it in `7e3c1b87d`.
