---
format: aep.planning-md/3
id: story:pod-removal-never-deletes-a-replacement
kind: story
status: active
title: A Pod removal never deletes a same-named replacement
relations:
- decomposes: epic:kubernetes-deployment-and-driver
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-29T08:57:33Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-29T08:57:33Z", actor: "human:timo", revision: 3}
---
# Story: A Pod removal never deletes a same-named replacement

## Outcome

Once the client's Pod removal has read the uid of the Pod it is removing, every later delete carries
that uid as a precondition, and a name that now holds another uid counts as the original Pod gone. The
removal never deletes, or reports as its own, a Pod it did not create.

## Finding

Adversary pass of 2026-10-29 on `wave/k8s-namespace-1` at `661d68e0`, scope client and executor, red
case `client_tests::a_removal_that_has_read_a_pods_uid_does_not_delete_a_same_named_replacement` on
`adv/k8s-client` (`26e077a8`), verified by the coordinator:

- `Api::remove_seen` in `crates/substrate-kubernetes-client/src/api.rs` notes each object it reads in
  its `PodTrace`, but `remove_once` deletes by name with no `preconditions.uid`, so the next attempt
  deletes a same-named replacement and reports the removal done.
- Callers are the Pod confinement adapter, `crates/substrate-host/src/confinement/pod.rs` (design 28).
  Its Pod names are adapter-minted ULIDs, so reaching it needs another party to create a Pod under
  that exact name; the namespace driver already deletes with uid preconditions.
- Design 28 says a failed start "deletes the Pod by name"; a uid precondition does not contradict it.

## Acceptance

- After the first read of the Pod, deletes carry its uid as a precondition; a 409 on the precondition
  or a read showing another uid ends the removal as gone without deleting that object.
- Before any read, behaviour is unchanged.
- The red case above passes; the Pod adapter's existing tests stay green.

## Design

Design 28 (accepted) and the uid rule of design 34 § 4. No contract change.

## Scheduling

Implemented within `wave/k8s-namespace-1` (operator, 2026-10-29: validated and confirmed adversary
findings get a story and are implemented within the current wave). Fix unit S.

## Changed adversary assertion (D188)

Fix unit S (`0935f0f9`) changed the return-value check of the adversary case
`a_removal_that_has_read_a_pods_uid_does_not_delete_a_same_named_replacement`
(`crates/substrate-kubernetes-client/src/client_tests.rs`). The coordinating session accepted it as
D188 on 2026-10-29. The wave's pull request body shows both versions side by side, so the change is
visible to review:

| before (adversary, `26e077a8`) | after (fix unit S, `0935f0f9`) |
|---|---|
| `assert!(!removed, "the removal reported gone a Pod it did not delete");` | `assert!(removed, "a name holding another uid is the Pod read gone (story:pod-removal-never-deletes-a-replacement)");` |
| none | `assert_eq!(seen.uid(), Some(first_uid.as_str()), "the removal took the replacement as its own Pod");` |

- The acceptance above says a name now holding another uid counts as the original Pod gone. The Pod
  adapter's `monitor()` waits for the removal to end, and `true` sends the end to the original uid.
- The check that the replacement is not deleted is unchanged.
- The follow-up `story:preconditioned-delete-409-is-not-a-uid-mismatch` covers a 409 caused by
  something other than a uid mismatch.
