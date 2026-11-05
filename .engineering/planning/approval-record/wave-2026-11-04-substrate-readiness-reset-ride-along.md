---
format: aep.planning-md/3
id: approval-record:wave-2026-11-04-substrate-readiness-reset-ride-along
kind: approval-record
status: approved
title: The wave's pull request takes the operational readiness reset fix as a ride-along
revision: 2
transitions:
- {from: "draft", to: "approved", at: "2026-11-05T01:57:39Z", actor: "human:timo", revision: 2}
---
# The wave's pull request takes the operational readiness reset fix as a ride-along

Decided by the orchestrator under its grant on 2026-11-05, after pull request 212's first CI run
(`approval-record:wave-2026-11-04-substrate-merge-and-release`).

## Decision

- The `Gate: tests and Clippy` job failed on
  `adversary_ops_listener::adversary_dispatch_counts_one_admission_and_no_replay`
  (`crates/substrate-daemon/tests/tls_listener.rs:1667`):
  `operational listener connection: Connection reset by peer (os error 104)`.
  The test's readiness helper `ops_when_ready` retried a refused connect but panicked on a reset
  one. The same test passed in both local gate runs.
- No rerun: it would leave the same reset in place for the next pull request.
- Ride-along in this wave: `ops_when_ready` treats `ConnectionReset` on connect like
  `ConnectionRefused`, inside its existing deadline. Only the targeted test target runs locally.
- The merge-on-green go holds for the new head. If the systemd netns floor test is then the only
  red check, the lead stops and reports instead of merging.
