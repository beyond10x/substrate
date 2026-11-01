---
format: aep.planning-md/3
id: story:kept-written-policy-log-names-the-write
kind: story
status: active
title: A kept written policy's log says it was written after its create
relations:
- informed_by: review-result:wave-2026-11-01-d354-u4-adversary-pass-1
scope:
- confidence: cited
  path: crates/substrate-host/src/confinement/pod.rs
- confidence: inferred
  path: crates/substrate-host/src/confinement/pod_cleanup_never_created.rs
- confidence: inferred
  path: crates/substrate-kubernetes-client/src/gone.rs
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-11-01T20:34:25Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-01T23:08:45Z", actor: "human:timo", revision: 6}
---
# Story: a kept written policy's log says it was written after its create

## Outcome

When the `pod` adapter keeps a `NetworkPolicy` because its metadata shows a write after its create,
its log line gives that reason, and the line appears once per policy and sweep where the policy is
old and its Pod gone, not on every open sweep for every written policy.

## Observation

Wave D354 adversary pass on U4 (`review-result:wave-2026-11-01-d354-u4-adversary-pass-1`, finding 2,
read from the code): the warn at `crates/substrate-host/src/confinement/pod.rs:483` carries the why
"no resourceVersion of the run was read to watch from" (`gone.rs:298`), and the info line at
`pod.rs:568` fires on every open sweep for every written policy, young or present ones included.

## Acceptance

- A kept written policy, old and with its Pod gone, logs one warn whose reason names the write after
  its create.
- A young written policy, or one whose Pod is present, logs no such line.

## Scope

Derived 2026-11-02 by `story-scoper` (wave D364). Every line is **cited** (read from the story or the tree) or **inferred** (a reading that could be wrong). Implemented in one unit with `story:written-policy-swept-when-shown-stopped`: both edit the same functions.

- **Crate:** `b10x-substrate-host`, cited. The reason text comes from `substrate-kubernetes-client`, cited.
- **Source:** `crates/substrate-host/src/confinement/pod.rs` `sweep` warn at 479-484 (message at 483), cited; the `resweep` arm at 734 reuses the same `why`, cited. Which warn the acceptance means, inferred.
- **Source:** `pod.rs` `sweep_page` info at 565-571 (message at 568) fires for every written policy whatever its age or Pod, cited.
- **Reason source:** `crates/substrate-kubernetes-client/src/gone.rs:298` (the story's `gone.rs`); the same text sits at gone.rs:204 (`pod_end`), cited.
- **Tests:** no log capture exists in `substrate-host` tests: no subscriber, no `tracing-test`, `tracing-subscriber` absent from its dev-dependencies (`Cargo.toml:46-49`), cited. Existing tests assert on the fake's request list (`pod_cleanup_never_created.rs:14-32`), cited. The only global-subscriber pattern is `crates/substrate-kubernetes/src/namespace/probe_tests.rs:1666-1681`, cited. Test approach (a pure reason function, or a scoped subscriber), inferred.
- **Would collide with:** any unit touching `pod.rs` sweep/resweep or `PodTrace`, cited.
- **Confidence:** high on locations; low on the log-test approach, which is the implementor's choice.
