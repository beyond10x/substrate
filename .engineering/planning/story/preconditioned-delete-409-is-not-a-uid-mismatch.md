---
format: aep.planning-md/3
id: story:preconditioned-delete-409-is-not-a-uid-mismatch
kind: story
status: active
title: A 409 on a preconditioned Pod delete is told apart from a uid mismatch
relations:
- decomposes: epic:kubernetes-deployment-and-driver
- depends_on: story:pod-removal-never-deletes-a-replacement
scope:
- confidence: cited
  path: crates/substrate-kubernetes-client/src/api.rs
- confidence: inferred
  path: crates/substrate-kubernetes-client/src/client_tests.rs
- confidence: inferred
  path: crates/substrate-kubernetes-client/src/fake.rs
revision: 9
transitions:
- {from: "draft", to: "proposed", at: "2026-10-30T17:39:13Z", actor: "human:timo", revision: 7}
- {from: "proposed", to: "active", at: "2026-10-30T17:39:13Z", actor: "human:timo", revision: 8}
---
# Story: A 409 on a preconditioned Pod delete is told apart from a uid mismatch

## Outcome

When a Pod delete carrying a uid precondition is answered 409, the client's removal reads the Status
`details` and GETs the Pod again before it decides. Only a name that holds another uid, or no object,
ends the removal as gone. Any other conflict is retried or reported, never taken as the original Pod
gone.

## Finding

Fix unit S of `wave/k8s-namespace-1` (`fix/pod-removal-uid`, `0935f0f9`) reported it, and the
coordinating session filed it on 2026-10-29:

- `remove_once` in `crates/substrate-kubernetes-client/src/api.rs` reads every 409 on a
  preconditioned delete as a uid mismatch and ends the removal as gone.
- Nobody checked whether the API server answers 409 on a Pod delete for any other cause. If one
  exists, the original Pod is reported gone while it still exists.

## Acceptance

- On a 409, the removal GETs the name; the Status does not decide, because an admission webhook
  sets its own Status on a denial and a 409's `reason` and `details` name no single cause:
  - another uid, or absent: gone, nothing deleted;
  - the same uid: the conflict was not a uid mismatch, and the removal tries again within its
    existing bound.
- A red case first, on the fake API: a 409 whose follow-up GET still shows the read uid.
- The unit S cases and the Pod adapter's tests stay green.
- The story cites the Kubernetes API reference for the causes of a 409 on a delete with
  preconditions:
  - DeleteOptions `preconditions`: "Must be fulfilled before a deletion is carried out. If not
    possible, a 409 Conflict status will be returned."
    (https://kubernetes.io/docs/reference/kubernetes-api/common-definitions/delete-options/#DeleteOptions)
  - Admission webhook responses carry a webhook-chosen `status` (code, reason, message), so a
    denial can arrive as a 409 that is not the precondition
    (https://kubernetes.io/docs/reference/access-authn-authz/extensible-admission-controllers/#response).

Amended during wave 2026-10-30-remote-conformance: the first bullet originally required parsing the
Status `reason` and `details`; the coordinator accepted the implementor's case that the GET alone
decides.

## Design

Design 28 (accepted) and the uid rule of design 34 § 4. No contract change.

## Scheduling

Draft, for the first substrate wave after `wave/k8s-namespace-1`, beside
`story:remote-clean-room-conformance` and `story:kubernetes-collisions-metric` (the coordinating
session, 2026-10-29).
