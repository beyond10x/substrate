---
format: aep.planning-md/3
id: story:configurable-exec-cpu-ceiling
kind: story
status: implemented
title: The exec CPU ceiling is configurable
relations:
- decomposes: epic:host-run-gates
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-10T00:48:48Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-10T00:48:48Z", actor: "human:timo", revision: 4}
- {from: "active", to: "implemented", at: "2026-10-10T00:48:59Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"test_result":1}}}
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

## Evidence

Implemented by commit `a43568ea7` (merged in https://github.com/beyond10x/substrate/pull/122 as
`a16b12b20`) and released in
[0.7.12](https://github.com/beyond10x/substrate/releases/tag/0.7.12): `HostConfig::exec_cpu_cores`
and the daemon's `--exec-cpu-cores N`, default 1.

- First criterion: `default_exec_cpu_ceiling_writes_exactly_the_quota_main_writes`
  (`crates/substrate-host/src/process.rs`).
- Second criterion: `exec_declaring_exactly_n_cores_gets_n_periods_under_a_ceiling_of_n`,
  `exec_declaring_more_than_the_ceiling_is_clamped_to_n_cores` and
  `exec_declaring_less_than_the_ceiling_keeps_its_derived_quota` (same file), plus
  `exec_cpu_quota_keeps_its_one_millisecond_floor_under_any_ceiling`.
- Third criterion: `host_open_refuses_an_exec_cpu_ceiling_of_zero`,
  `host_open_refuses_an_exec_cpu_ceiling_above_the_usable_cpus` and
  `host_open_admits_the_default_and_every_usable_cpu_count`
  (`crates/substrate-host/tests/exec_cpu_ceiling.rs`); the daemon refuses at startup as
  `config.exec-cpu-cores-invalid`, and
  `exec_cpu_cores_defaults_to_one_core_and_takes_an_explicit_count`
  (`crates/substrate-daemon/src/main.rs`) covers the flag.
- Full gate green on the merge commit:
  https://github.com/beyond10x/substrate/actions/runs/38001895521, and on the pull request's head:
  https://github.com/beyond10x/substrate/actions/runs/38001031105.
- Release run https://github.com/beyond10x/substrate/actions/runs/38003936799 succeeded from tag
  `0.7.12` at `7d268e592`.
