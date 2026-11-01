---
format: aep.planning-md/3
id: review-result:wave-d339-u4-adversary-pass-1
kind: review-result
status: active
title: Adversary pass 1 of wave D339, unit U4
relations:
- reviews: story:pod-orphaned-policy-is-swept
revision: 1
---
# Adversary pass 1 of wave D339, unit U4

Agent type `aep:adversary`, against the story's Acceptance only. Tree `substrate-d339-adv` at
`b0c433d4`, unit commit `7cbc0cc4`. One case written; `cargo test -p
b10x-substrate-kubernetes-client`: 47 passed, 1 failed; host lib 403 and `adversary_pod` 22 passed.

```findings
- file: crates/substrate-kubernetes-client/src/gone.rs
  line: 375
  category: boundary
  severity: note
  verdict: INFEASIBLE
  origin: introduced
  message: pod_history returned NeverCreated on any BOOKMARK without reading its resourceVersion, so a BOOKMARK at the start version, an older one, or none was taken as a complete replay; only a fake reaches this.
```

Held: an event of the name with another uid, or a MODIFIED before an ADDED, marks the name seen
and keeps the policy; an ERROR event, a non-JSON line, the byte bound, a 410 as status or event and
a stream that ends each keep it; the client bound (8 s) exceeds the server's timeout (5 s); kept and
deferred reasons are redacted; the rule applies only when no Pod object of the run was ever read,
in `sweep` and in `resweep`.

Out of scope, one line each: the policy's `resourceVersion` is its last write, so another writer's
change after the Pod's create would start the replay past the Pod's history; a watch-cache lag
could put a just-deleted Pod's events after the BOOKMARK; a Pod stored after the client's create
timeout near the age floor; two mutants (REPLAY_LIMIT equal to WATCH_LIMIT, MODIFIED dropped from
the seen events) no fake can show.

Coordinator decision (D339, 2026-11-01):

- `gone.rs:375`: fixed in the wave (`fix(kubernetes-client): a bookmark at the start version
  proves no replay`): a BOOKMARK shows the replay complete only with a `resourceVersion` other
  than the start version; the case covers the start version and none (versions are opaque, so an
  older one is not told apart). Green: client 48, host lib 403, `adversary_pod` 22.
- Last-write `resourceVersion`: story `story:pod-sweep-replays-from-the-policys-create`
  (proposed); design 28 states the single-writer assumption.
- Watch-cache lag: no change. The event that matters is the Pod's ADDED, at least the 90 s age
  floor old, not one just before the read; a live adapter creating a Pod seconds ago is what the
  age floor excludes.
- Late Pod create near the age floor: no change; it belongs to the existing age floor, not this
  unit.
- The two mutants: no change; neither can be shown by the fakes, and the manual kind case passed
  with the server's real BOOKMARK timing.
