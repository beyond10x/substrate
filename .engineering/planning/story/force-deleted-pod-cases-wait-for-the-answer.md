---
format: aep.planning-md/3
id: story:force-deleted-pod-cases-wait-for-the-answer
kind: story
status: active
title: The force-deleted Pod cases mark the Pod gone only after the fake answered
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-10-30T23:39:43Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-30T23:39:43Z", actor: "human:timo", revision: 3}
---
# Story: The force-deleted Pod cases mark the Pod gone only after the fake answered

## Outcome

The cases in `crates/substrate-host/src/confinement/pod_cleanup_force_deleted.rs` hold under load:
the run's task has read its Pod running before the case marks it gone, so the task watches it from
the `resourceVersion` it read, as the cases assert.

## Finding

The delegated confinement lane (`bash scripts/delegated-lane.sh`) for release 0.17.0 failed at
`pod_cleanup_force_deleted.rs:155` twice, on two commits, a different case each time
(`a_watch_that_stays_silent_keeps_the_policy` at `1dc35c9e`,
`a_pod_force_deleted_from_an_unreachable_node_keeps_its_policy` at `28bf24fe`). Both passed in the
other run and in `cargo test -p b10x-substrate-host --lib`. The request log of the second failure
holds one `GET` of `substrate-run-cleanup`, its `DELETE` and no watch. `follow()` sets `pod_gone`
as soon as the `GET` is recorded, which the fake does before it answers, so under load the first
read can come back gone.

## Acceptance

- `follow()` marks the Pod gone only after the fake has answered the task's first read with the Pod
  running (an answered-read record or an equivalent signal the fake gives), never on the request
  alone.
- No assertion of the file changes; no bound in it grows.
- A red case first: the race forced deterministically (the fake delays its answer to the first
  read), failing before the fix and passing after.
- `cargo test -p b10x-substrate-host --lib` and the delegated lane pass.
