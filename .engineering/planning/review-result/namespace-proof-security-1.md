---
format: aep.planning-md/3
id: review-result:namespace-proof-security-1
kind: review-result
status: active
title: Security review of the namespace driver's proof holding, pass 1
relations:
- reviews: story:namespace-driver-start-holds-its-proof-until-pod-create
revision: 1
---
# Security review of the namespace driver's proof holding, first pass

Reviewed: branch `story/namespace-proof` at `52a1b0ec6`, by an independent reviewer on
2026-11-22. Six findings, all closed in `ee0af5a23`.

The two that changed the rule: the first commit compared the digest of the admission with the
current digest, which refuses every start that waited across a probe; and proof validity read
the wall clock. The rule is now design 34 § 6.3: facts, node, Lease epoch, binding and policy
set at the last comparison before the Pod create, with validity on the monotonic clock.

After the fixes: the kubernetes library ran 195 cases (186 before), the daemon's
`namespace_admission`, `contract_vectors` and `runtime_vectors` 51. Four new cases were red
against the driver code of `52a1b0ec6`. Mutants of each arm of the comparison were killed by a
case of its own, in three builds.

```findings
[
{"file":"crates/substrate-kubernetes/src/namespace/probe.rs","line":487,"category":"boundary","severity":"warning","verdict":"CONFIRMED","origin":"pre-existing","message":"Proof validity and the probe schedule read the wall clock, so a proof's validity followed the wall clock and not the time that passed. Closed: validity and the schedule are measured on the monotonic clock, and the wall clock can only end a proof early."},
{"file":"crates/substrate-kubernetes/src/exec.rs","line":333,"category":"judgement","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"The first rule compared the digest of the admission with the current one, so a start queued behind a waiting run across a probe was always refused 422 and its retry queued again. Closed: the start is held to facts, node, Lease epoch, binding and policy set; the digest is looked up, not compared."},
{"file":"crates/substrate-kubernetes/src/exec.rs","line":232,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"A comment said nothing waits before the create is sent; the create's own connect, TLS and HTTP exchange are awaited after the last comparison. Closed: the comment names them and their 30 s bound."},
{"file":"crates/substrate-kubernetes/src/exec.rs","line":1461,"category":"mutant","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"No case failed when the family arm or the whole-tree-kill arm of the required-facts comparison was removed. Closed: two cases hold a probe that proves on a node without one required fact."},
{"file":"crates/substrate-kubernetes/src/lib.rs","line":100,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"The publication-order field, its lock, its harness and its cases backed no production read. Closed: removed."},
{"file":"crates/substrate-kubernetes/src/exec_tests.rs","line":2426,"category":"judgement","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"A test name and two comments stated the precondition of the earlier behaviour instead of the rule. Closed: renamed and reworded."}
]
```
