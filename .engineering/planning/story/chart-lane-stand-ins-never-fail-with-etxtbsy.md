---
format: aep.planning-md/3
id: story:chart-lane-stand-ins-never-fail-with-etxtbsy
kind: story
status: active
title: Chart lane stand-in scripts never fail a test with ETXTBSY
relations:
- informed_by: approval-record:d342-substrate-wave-2026-11-01
scope:
- confidence: cited
  path: xtask/src/chart_cluster.rs
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-31T19:46:59Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-01T05:07:16Z", actor: "human:timo", revision: 3}
---
# Story: chart lane stand-in scripts never fail a test with ETXTBSY

## Outcome

`cargo test -p xtask` never fails because a stand-in script it just wrote is still open for
writing in another test's forked child. The unit tests in `xtask/src/chart_cluster.rs` pass under
the default parallel test runner on every run.

## Evidence

Main's Gate run 41234221370 (job 124634495582) at `620e89ce`, 2026-10-31:
`chart_cluster::tests::a_failed_helm_uninstall_is_a_removal_failure` panicked at
`xtask/src/chart_cluster.rs:4342` with `helm uninstall of the release: starting … helm … Text file
busy (os error 26)`; 450 other tests passed. The same tree was green on its pull request, and the
change did not touch `xtask`.

## Design

`stand_in` (`xtask/src/chart_cluster.rs`) writes a script with `fs::write` and makes it executable.
If another test forks while that write descriptor is open, the child holds it until its `exec`, and
an `exec` of the script in that window fails `ETXTBSY`. Fix proposed by the orchestrator: write to a
temporary name, then rename it into place. Inferred, not verified: a rename keeps the same inode,
so it closes the window only if the write descriptor is closed before any fork can copy it. The
alternative is to retry the spawn on `ETXTBSY` (`io::ErrorKind::ExecutableFileBusy`) a bounded
number of times. The implementing story picks one and proves it.

## Acceptance

- A test that starts a stand-in while another thread forks in a loop passes repeatedly.
- `chart_cluster::tests::a_failed_helm_uninstall_is_a_removal_failure` and the other stand-in tests
  keep their assertions.
