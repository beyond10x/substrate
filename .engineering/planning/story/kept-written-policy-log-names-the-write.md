---
format: aep.planning-md/3
id: story:kept-written-policy-log-names-the-write
kind: story
status: proposed
title: A kept written policy's log says it was written after its create
relations:
- informed_by: review-result:wave-2026-11-01-d354-u4-adversary-pass-1
revision: 2
transitions:
- {from: "draft", to: "proposed", at: "2026-11-01T20:34:25Z", actor: "human:timo", revision: 2}
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
