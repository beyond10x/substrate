---
format: aep.planning-md/3
id: story:gate-test-step-ends-within-two-minutes
kind: story
status: implemented
title: The gate's test step ends within two minutes and no test runs over 60 s
summary: 'Hosted: 1126 s with compile, 354 s of tests, one test over 60 s; the runner enforces the budget'
tags:
- defect
- gate
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-11-21T21:01:49Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-11-21T21:01:49Z", actor: "human:timo", revision: 4}
- {from: "active", to: "implemented", at: "2026-11-22T03:10:48Z", actor: "agent:host-operator", revision: 6, decided_on: {"recorded":{"test_result":1}}, executor: "agent:substrate"}
---
# Story: the gate's test step ends within two minutes and no test runs over 60 s

## Outcome

`cargo test --workspace --release --locked`, the test step of `scripts/gate.sh`, ends within 2
minutes of wall time on a warm build, no single test runs longer than 60 s, the runner kills a
test at 60 s and the step prints its ten slowest tests.

## Where the requirement comes from

The operator's rule of 2026-11-21 for every repository of the collection, section *Tests stay
fast* of the collection folder's `AGENTS.md` on the operator's workstation: the test step of a
gate ends within 2 minutes, no test over 60 s, an expensive product is made once and never per
test, full regeneration runs in a lane named `slow`, and a step over the budget gets a story that
is fixed before the next feature. Stakeholder: the operator.

## Measured

Hosted run of `gate.yml` on `main` at `597a495cd` (run 44946254525, job *Gate: tests and
Clippy*, 2026-11-21):

- the step from `gate: cargo test --workspace --release --locked` to the next step: 20:29:37 to
  20:48:23 UTC, 1126 s, compile included on a cold runner;
- 188 `test result:` lines whose `finished in` times sum to 354.1 s;
- the slowest test binary finished in 95.65 s, the next four in 53.43 s, 36.64 s, 32.61 s and
  17.87 s; their names were not read for this draft;
- one test was reported `has been running for over 60 seconds`:
  `exec_memory_limit_refusal_is_named`.

Not measured: the step on the workstation with a warm build, and any single test's own time
(libtest prints none on the pinned toolchain).

## What changes

1. Measure first: the step on the workstation, warm, and each test's time, with a runner that
   prints them. The ten slowest are named in the story's evidence.
2. The runner enforces 60 s per test and prints the ten slowest. cargo-nextest with
   `slow-timeout` and `terminate-after` is the form the rule names; adopting it changes
   `scripts/gate.sh`, `gate.yml` and `AGENTS.md` § *The gate*, and doctests need their own step.
3. Each test over 60 s, `exec_memory_limit_refusal_is_named` first, is made to finish under it or
   is split so that the expensive product it builds is made once per binary.
4. What cannot be made fast and regenerates a committed product from its sources moves to a lane
   named `slow` that runs when those sources changed.
5. The compile is not the test step's budget; the story states how the two are told apart in the
   gate's output.

## Read from the log, not yet measured per test

`exec_memory_limit_refusal_is_named` is in `crates/b10x-substrate-cli/tests/adversary.rs`, the
binary that finished in 95.65 s with 3 tests. While it ran, the log holds cargo's
`Finished release profile … in 1m 35s`. Inferred: a test of that binary builds something with
cargo, which is the rule's named mistake, an expensive product made inside a test.

## Decided

The operator session decided on 2026-11-21
(`approval-record:host-operator-orders-the-test-budget-story-2026-11-21`): the story is built
now, in its own worktree beside `story:executable-deny-list`, and no feature story starts after
that one until this is merged. The budget is held on the hosted runner.

## Not decided

- Whether the release lanes (`scripts/delegated-lane.sh`, quota, chart, namespace) are inside the
  rule. They are lanes and not the gate's test step; the delegated lane's wall time was not
  measured for this draft.

## Acceptance

The operator session holds the story to this:

- the gate's test step ends within 120 s of wall time on the hosted runner; the report states how
  compile time is counted and gives both numbers, with and without compile;
- no test runs over 60 s, and the runner kills one that does: a test made to sleep 61 s is killed
  and fails the step;
- the step itself fails when it runs over 120 s, locally and in CI (added on 2026-11-21 by the
  operator's order to the collection, relayed by the operator session: the collection rule's
  line "The step itself fails when it runs over 120 s"); the compile is outside that bound and
  the gate's output shows both times;
- the step prints its ten slowest tests;
- `exec_memory_limit_refusal_is_named` finishes under 60 s, or is in a lane named `slow` with its
  reason;
- no test is deleted or ignored to reach this without being named in the report;
- the before and after times are recorded as evidence with the run they were read from.
