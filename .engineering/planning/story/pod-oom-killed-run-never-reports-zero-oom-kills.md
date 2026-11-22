---
format: aep.planning-md/3
id: story:pod-oom-killed-run-never-reports-zero-oom-kills
kind: story
status: draft
title: A pod run killed at its memory bound never reports zero OOM kills
summary: 'Hosted Pod lane, once: exit 137 beside memory_oom_kills Some(0); the read can precede the count'
tags:
- defect
- flaky
revision: 1
---
# Story: a pod run killed at its memory bound never reports zero OOM kills

## Outcome

On the `pod` adapter, a run that the kernel killed at its memory bound reports
`memory_oom_kills` of at least 1, or reports the count as absent with its reason. It does not
report `Some(0)` beside exit code 137.

## Where the requirement comes from

The hosted `Pod lane` job of pull request 249 failed once on 2026-11-22 in
`a_run_over_its_memory_bound_is_oom_killed_and_leaves_nothing`
(`crates/substrate-host/tests/ess_confinement_pod.rs:1028`), run 44960724452, job 137018742447.
The pull request changes workflows only. The same job passed on pull request 250 and on `main`
at `acc9ecf10` the same night. Stakeholder: session `runbook`, whose consumer reads limit hits.

## Observed

```
assertion `left == right` failed: RunObservation { …, adapter: Pod, …, state: Ended,
exit: Some(RunExit { code: Some(137), signal: None }), memory_oom_kills: Some(0) }
  left: Some(0)
 right: Some(1)
```

The run ended with code 137, and the count the adapter read was 0. Seen once in three runs of
that job that night. Not reproduced locally; the lane needs a kind cluster.

## Why it matters

- `memory_oom_kills` is what a consumer reads to tell a memory-bound kill from another kill.
  `Some(0)` says "observed, and no kill", which is wrong here; invariant 3 asks for an absent
  fact over an optimistic one.
- The limit stories of `epic:runs-are-governed-and-limit-hits-are-events` build on this count.

## Not known

- Where the count is read from on `pod` and when (inferred: the container's cgroup `memory.events`
  or the container status, read before the kill is counted, or after the cgroup is gone).
- Whether the same read can be early on `linux` or `oci`.

## What changes

1. Find the read and why it can precede the count: read the adapter's source, then make the case
   fail on demand (a read forced before the container's termination is observed).
2. The adapter reads the count only once the kill is observable, or reports it absent when it
   cannot. Decide which and state it; the wire is not touched.
3. The lane case keeps its assertion. It is not loosened to accept 0.

## Acceptance

A case that forces the early read is red before the change and green after it, and the hosted
`Pod lane` reports `memory_oom_kills: Some(1)` for the memory-bound case in the change's own run.
