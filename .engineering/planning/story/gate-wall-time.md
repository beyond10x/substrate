---
format: aep.planning-md/3
id: story:gate-wall-time
kind: story
status: implemented
title: The full gate's wall time falls without dropping a check
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-28T21:27:15Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-28T21:27:16Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-28T21:27:17Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}}
---
# The full gate's wall time falls without dropping a check

## Why

Run 40753683650 on `main`: the `Full gate` job takes 20m59s, and because the ruleset requires an
up-to-date branch every queued PR pays it again. `cargo test --workspace --release` takes 14m53s of
it: 9m00s compiling 114 test binaries, 1m24s for a nested daemon build started by
`crates/b10x-substrate-cli/tests/adversary.rs`, and 2m02s in `adversary_pod`, which waits out real
two-minute and 92 s windows. The static `cargo xtask` checks (check-ess, check-json, check-licenses,
check-bundles, check-secrets and the rest) take about 4m40s after the tests, in series.

## Change

1. The static checks run in a second CI job beside the tests. `scripts/gate.sh` keeps running every
   step locally; CI calls it per phase. The required context `Full gate` still exists and is green
   only when both phases are.
2. The Pod adapter's timing windows that `adversary_pod` waits out become injectable for tests, so
   each scenario runs in seconds with the same assertions.
3. The CLI adversary test reuses a daemon binary the gate already built, where that is the same
   binary the nested build would produce.

## Acceptance

- Every step `scripts/gate.sh` runs today still runs on every push and pull request.
- The `Full gate` context gates `main` and `release.yml` as before.
- No adversary assertion is removed or loosened.
- A recorded CI run shows the new wall time against 20m59s.
