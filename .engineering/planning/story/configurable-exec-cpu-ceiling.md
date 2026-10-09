---
format: aep.planning-md/3
id: story:configurable-exec-cpu-ceiling
kind: story
status: draft
title: The exec CPU ceiling is configurable
relations:
- decomposes: epic:host-run-gates
revision: 1
---
# Story: the exec CPU ceiling is configurable

## Problem

`ProcessCgroup::create` (`crates/substrate-host/src/process.rs`) derives `cpu.max` from
`cpu_millis / timeout_ms` and clamps the quota to one period, so every exec is held to one core. A
build that declared enough CPU time for several cores still runs on one.

## Design

`HostConfig` gains an operator-set ceiling in whole cores (default 1, the current behaviour). The
quota keeps its derivation from the declared limits and is clamped to `cores × period` instead of
one period. The daemon exposes the ceiling as a flag. No wire byte changes: the capability fact
does not state the clamp today and still does not.

## Acceptance

- With the default configuration, an exec's `cpu.max` is exactly what it is on `main`.
- With a ceiling of N cores, an exec declaring `cpu_millis` equal to N × `timeout_ms` gets
  `cpu.max` of `N×100000 100000`; one declaring more is clamped to N cores; one declaring less keeps
  its derived quota.
- A ceiling of 0 or above the host's online CPU count is refused at configuration time.
