---
format: aep.planning-md/3
id: review-result:adr-0109-u10-adversary-pass-1
kind: review-result
status: active
title: 'Adversary pass 1: sandbox repository parity'
relations:
- reviews: story:sandbox-repository-parity
revision: 1
---
# Adversary pass 1: sandbox repository parity

Reviews `story:sandbox-repository-parity` at `f6f125b8` (merged as `e2e38489`). Red cases:
`crates/b10x-substrate-cli/tests/confine_adversary.rs`. Executed 12 before, 19 after; 6 red (2
portable, 4 delegated).

## Findings

| # | file:line | severity | finding |
|---|---|---|---|
| 1 | `crates/b10x-substrate-cli/src/confine.rs:254` | blocker | Every `confine run`, even `--dry-run`, opens a host driver on the shared `SUBSTRATE_CGROUP_ROOT` (the daemon's variable). Its orphan reconcile SIGKILLs other runs' `substrate-ex_*` groups. Observed: the first run exited 137. |
| 2 | `confine.rs:174` | warning | The verb sets only a non-sandbox `PATH`; `HOME`, `LANG`, `LC_ALL` and `TZ` are missing. The parity rows pass only on the test's own environment. |
| 3 | `crates/substrate-host/src/probe.rs:302` | warning | The open-time probe inherits stderr, so two probe lines precede the command's stderr or the JSON envelope. |
| 4 | `confine.rs:149` | warning | `--dir` refuses names outside `[A-Za-z0-9_-]+` as `workspace.path-escape`. |
| 5 | `confine.rs:217` | note | A relative `--ro` with `..` is refused as not canonical. |
| 6 | `confine.rs:187` | note | `--dry-run` exits 0 where the run is refused, and prints render-local descriptor numbers. |
| 7 | `scripts/delegated-lane.sh:111` | note | No lane runs a CLI delegated case. |
| 8 | `confine.rs:275` | note | Fixed bounds, no stdin, output captured. |
| 9 | `confine.rs:69` | note | `argv` is required; the O6 dry run passes none. |

Exclusions judged:
- The six unmirrored flags are justified for the one-directory layout.
- The ESS forms are justified.
- The delegated root is justified as a floor, but it hid finding 1.

## Decisions taken by the coordinator

- 1: the CLI reads its own variable, not the daemon's. Each invocation uses a private child cgroup, or else a named refusal when the root holds other groups.
- 2: set sandbox's five variables; parity asserts them from the verb's render.
- 3: probe stdio to null (a `substrate-host/src` change, accepted).
- 4: keep the host's base-name rule; the CLI refuses first, with a message that names the rule.
- 5: normalize lexically.
- 6: a dry run runs the same admission and says the descriptor numbers are render-local.
- 7: add the CLI delegated cases to the lane.
- 8, 9: no change; named in `--help`. Item 9 waits on https://github.com/beyond10x/ess/issues/694.
