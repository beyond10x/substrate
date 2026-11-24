---
format: aep.planning-md/3
id: story:a-cpu-budget-is-declared-apart-from-a-cpu-rate
kind: story
status: draft
title: A CPU budget is declared apart from a CPU rate
relations:
- decomposes: epic:runs-are-governed-and-limit-hits-are-events
revision: 1
---
# Story: a CPU budget is declared apart from a CPU rate

## Outcome

A profile, and an exec start, can bound the CPU time a run may use in total without also
bounding the share of a CPU it may use at any moment, and can bound that share on its own.

## Where the requirement comes from

Session `loom`, the first consumer of `substrate_host::confinement`, on 2026-11-24, in its
answer to the questions of design 36: a program run with a CPU budget of 3,600,000 ms and a
timeout of 14,400 s runs at a quarter of a core for its whole life. The stakeholder of this
story is that session.

## What holds today

`exec_cpu_quota` in `crates/substrate-host/src/process.rs` writes `cpu.max` for every exec: the
declared `cpu_millis` spread over the declared `timeout_ms`, at least 1 ms in each period of
100 ms, at most `HostConfig::exec_cpu_cores` whole periods. The cumulative budget is held
beside it and ends the run (`exec.cpu-limit`). The rate is not published: its documentation
says why (`exec.cgroup-limits` is a closed object in every released bundle).

## What is open

This is a draft. Nothing is decided.

- Whether the rate stays derived, becomes a declared bound of its own (`ProcessBounds`, the
  exec start's `limits`), or is the operator's ceiling alone.
- What a start that declares no rate gets: the derived rate as today, or the ceiling.
- The wire: a new member of the exec start's `limits` and of the `exec.cgroup-limits` fact is a
  successor bundle and needs an ADR before code (invariants 6 and 8).
- `oci` and `pod` bound a rate and no budget (design 36, "Not decided here").

## Boundaries

- No change to the cumulative budget or to the code a run ended by it is recorded with.
- Design 36 does not depend on this story.
