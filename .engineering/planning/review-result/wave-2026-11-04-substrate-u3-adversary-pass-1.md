---
format: aep.planning-md/3
id: review-result:wave-2026-11-04-substrate-u3-adversary-pass-1
kind: review-result
status: active
title: Wave 2026-11-04 substrate U3 adversary pass 1
relations:
- reviews: story:namespace-lane-same-named-case-creates-its-workspace
revision: 1
---
# Adversary pass 1 — wave 2026-11-04 U3 (story:namespace-lane-same-named-case-creates-its-workspace)

Diff `a692acb7...18d18203` (unit commit `0a9af044`); adversary commit `8848602f` on `adv/2026-11-04-u3`, merged into the integration branch at `10f04a62`. Agent `aep:adversary`. Findings verbatim; prose trimmed to the measured facts.

```
verdict: nothing found in the implementation
cases: executed 29→34, red 0
origin: introduced 2, pre-existing 0, undecided 0
```

Commands: `cargo test -p xtask --bin xtask namespace_lane_run` 34/0; `cargo fmt --all --check` exit 0; clippy `-D warnings` exit 0.

- The copy of the conformance crate's `settled` matches: 500 ms pause, `RUNNER_RESTART` bound, terminal and refused as ends, the create's own operation id.
- New cases, all green:
  - a 201 create;
  - a refusal, and a non-retriable outcome-unknown;
  - an operation that never ends;
  - an operation read answered 404 or 500;
  - a success that names no workspace.
- Mutant A (five mutations) turned all five new cases red, while the unit's two tests stayed green.
- Treating `unknown` as an end turned the unit's tests red, and so did skipping the wait in `destroy_completes`.

Resolution: both findings are fixed by the cases merged at `10f04a62`.

```findings
[{"file":"xtask/src/namespace_lane_run.rs","line":947,"category":"mutant","severity":"warning","verdict":"CONFIRMED","origin":"introduced","message":"scope in; Nothing the unit tested covered the settle deadline: with it removed, an operation that never ends loops forever; step_six_gives_up_at_the_bound_when_the_operation_never_ends now covers it."},
 {"file":"xtask/src/namespace_lane_run.rs","line":885,"category":"mutant","severity":"note","verdict":"CONFIRMED","origin":"introduced","message":"scope in; Mutations to the 201 branch (885), the retriable guard (890), the empty-id check (897) and the status check on the operation read (922) all passed the unit's two tests; four new cases now cover them."}]
```
