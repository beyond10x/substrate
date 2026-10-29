---
format: aep.planning-md/3
id: review-result:design-34-u6-security-pass-1
kind: review-result
status: active
title: Security review of design 34 U6, the namespace driver core, pass 1 (2 warnings, 4 notes)
relations:
- reviews: story:kubernetes-namespace-workspace-and-exec-slice
revision: 1
---
# Security review of design 34 U6, the namespace driver core, pass 1

Reviewed commit `1e4ebfdc` on `wave/k8s-namespace-1`. Red cases: `e67f1878` on
`sec/u6-driver-core` (`crates/substrate-kubernetes/src/driver_tests.rs`, +73 lines, no
implementation file touched). Crate tests 12 before, 14 after, 2 red.

Verdict: NEEDS-CHANGE. No blocker, 2 warnings, 4 notes.

## Findings

```findings
- {file: crates/substrate-kubernetes/src/namespace.rs, line: 362, category: acceptance, severity: warning, verdict: NEEDS-CHANGE, origin: introduced, message: "check_binding compares against the latest open's binding, so a re-open over a changed policy set lets an earlier admitted start pass"}
- {file: crates/substrate-kubernetes/src/namespace.rs, line: 396, category: integrity, severity: warning, verdict: CONFIRMED, origin: introduced, message: "remove_owned settles an unbound row on first absence, orphaning a create that lands later"}
- {file: crates/substrate-kubernetes/src/rules.rs, line: 157, category: boundary, severity: note, verdict: CONFIRMED, origin: introduced, message: "excess grows as the product of a rule's lists, unbounded memory from a small Role"}
- {file: crates/substrate-kubernetes/src/lease.rs, line: 126, category: judgement, severity: note, verdict: CONFIRMED, origin: introduced, message: "an empty-holder Lease labelled for another deployment is taken and keeps the foreign label"}
- {file: crates/substrate-kubernetes/src/ownership.rs, line: 46, category: judgement, severity: note, verdict: INFEASIBLE, origin: introduced, message: "the operation annotation is copyable from the sibling PVC to adopt a forged keeper, needs namespace write"}
- {file: crates/substrate-kubernetes/src/namespace.rs, line: 358, category: concurrency, severity: note, verdict: CONFIRMED, origin: introduced, message: "the Lease is checked before a policy GET that can outlast it"}
```

## Red cases

- `a_start_admitted_before_a_reopen_over_a_changed_policy_set_is_stale`: panics at
  `driver_tests.rs:854`, the check returns Ok.
- `an_unbound_row_read_absent_still_removes_an_object_whose_create_lands_later`: first removal
  Absent, second Collision.

## Checked and sound

- Lease: no takeover before 30 s unchanged, never of a foreign holder; every update conditional on
  the version read; renewal time taken before the request is sent.
- Rules review matches decision 3 and Q1 B: wildcards, `secrets`, subresources, wrong case and
  unreadable rules count as excess.
- Policy digest covers name, uid, resourceVersion and spec; lists are bounded and a truncated
  list is refused.
- The keeper name is 40 characters; design 34 § 4's 39 is a document slip (U13).
- API messages are redacted; debug output carries no token.

## Not verified

A real cluster's rules review and Pod Security message text (the lane's job), two instances of one
deployment, and the U9 to U11 callers, which do not exist yet.

## Disposition

Warnings 1 and 2 and notes 3, 4 and 6 go to fix unit H on this wave. Note 5 is accepted under
design 34 decision 4: forging it needs the rights the driver itself holds.
