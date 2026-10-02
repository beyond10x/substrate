---
format: aep.planning-md/3
id: review-result:live-output-stall-adversary-r1
kind: review-result
status: active
title: Independent queue cancellation review, regression and correction
relations:
- reviews: story:live-output-stall
revision: 1
---
unit: Substrate output stall 20c96f79 plus reviewed correction  
verdict: CONFIRMED finding, corrected and retested  
cases: executed 29→30, initially red 1, currently red 0  
origin: introduced 0 / pre-existing 0 / undecided 1  
wrote-outside-worktree: assigned review logs  
lease: released; target returned to implementor

```text
crates/substrate-host/src/process.rs | 85 ++++++++++++++++++++++++++++++++++++
1 file changed, 85 insertions(+)
```

Of those lines, 80 are my authorized additions inside `#[cfg(test)]`; the implementor added the five-line production correction.

The new regression demonstrated that a pending stderr reservation could enqueue after stdout’s stall deadline had already set terminal backpressure. It drives the actual `drain_capped` seam with shared queue/execution state, matching production raw-pipe stdout/stderr ownership.

First execution:

```text
terminal backpressure admitted sibling payload
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 121 filtered out
```

The implementor added an Acquire check after reservation and before counters or payload copying. I verified that no test assertion was weakened, then independently reran the process suite:

```text
test result: ok. 30 passed; 0 failed; 0 ignored; 0 measured; 92 filtered out
```

The corrected case covers recorded and unrecorded modes, exact capture behavior, observed/queued byte counters, and queue occupancy. This pass does not establish full-gate, delegated-lane, or Mantle production acceptance.

Portable evidence destinations supplied to the coordinator:

- `.engineering/reports/live-output-stall-2026-10-03/shared-stall-red.log`
- `.engineering/reports/live-output-stall-2026-10-03/process-green.log`

| Location | Finding | Result |
|---|---|---|
| `crates/substrate-host/src/process.rs:2870` | Pending sibling stream could resume forwarding after terminal backpressure. | CONFIRMED; corrected and regression green. Origin undecided because the prior synchronous implementation was not executed with an equivalent concurrency test. |

```findings
- file: crates/substrate-host/src/process.rs
  line: 2870
  category: concurrency
  severity: note
  verdict: CONFIRMED
  origin: undecided
  message: Pending sibling stream could resume forwarding after terminal backpressure; the implementor added a post-reservation guard and the retained regression now passes.
```
