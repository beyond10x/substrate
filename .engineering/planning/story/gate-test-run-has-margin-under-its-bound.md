---
format: aep.planning-md/3
id: story:gate-test-run-has-margin-under-its-bound
kind: story
status: draft
title: The gate's test run has margin under its 120 s bound on the hosted runner
revision: 1
---
# Story: the gate's test run has margin under its 120 s bound on the hosted runner

## Outcome

The test run of the gate ends well before the 120 s at which the runner kills it, on the hosted
runner, so that a slower runner or the next 200 tests do not fail the gate. On 2026-11-24 it had
6 s left on `main` and 1 s left on one run of a pull request.

## Where the requirement comes from

The collection's rule *Tests stay fast* (the test step of a gate ends within 2 minutes; a step
over the budget is a defect that is fixed before the next feature) and this repository's
`story:gate-test-step-ends-within-two-minutes`, which set the bound. Stakeholder: the operator.

## What was measured, 2026-11-24

`gate-time: test run` of the last six completed gate runs on `main`, oldest first, read from
the job logs: 105 s, 100 s, 99 s, 108 s (run 45364388174), 109 s, 115 s (run 45397070673, at
`db15951fb`). The same line on pull request 295, which adds about 200 tests: 88 s at
`b3cf567ed` (3743 tests), 119 s at `1a566aec0` (3764 tests) and 100 s at `7ef5cff97` (3770
tests). The test build of those three runs took 586 s, 848 s and 631 s, so the second ran on a
slower runner.

At `db15951fb`: cargo-nextest's summary 113.8 s for 3561 tests whose times sum to 1303.6 s.
With 12 test threads that sum fills 108.6 s when every thread is busy the whole time. The run
is therefore bound by the sum of the test times, not by one long test.

The ten slowest of that run (`xtask slowest-tests`):

| seconds | binary | case |
|---|---|---|
| 40.6 | `xtask::host_no_default_graph_security_review_u1` | `a_git_edge_below_the_host_reaches_its_no_default_graph_and_check_packages_refuses_it` |
| 40.5 | `xtask::executor_tree_rule_adversary_u4_pass1` | `a_musl_only_edge_to_a_fenced_crate_is_refused_by_the_executor_tree_rule` |
| 36.8 | `b10x-substrate-daemon::hosted_pool_adversary_pass1` | `adversary_idle_connection_older_than_the_idle_timeout_is_closed` |
| 25.1 | `b10x-substrate-host` | `process::tests::every_probe_sandbox_carries_the_user_namespace_posture` |
| 23.4 | `xtask::kubernetes_client_fake_fence_adversary` | `the_unmutated_manifests_pass` |
| 22.7 | `xtask::kubernetes_client_fake_fence_adversary` | `a_fake_edge_in_a_path_manifest_outside_the_root_is_refused` |
| 22.5 | `xtask::kubernetes_client_fake_fence_adversary` | `a_fake_dev_edge_in_a_member_with_a_binary_target_is_refused` |
| 22.2 | `xtask::kubernetes_client_fake_fence_adversary` | `a_fake_edge_in_an_unlisted_path_member_is_refused` |
| 22.2 | `xtask::kubernetes_client_fake_fence_adversary` | `a_fake_dev_edge_in_a_library_member_passes` |
| 19.7 | `b10x-substrate-kubernetes` | `namespace::probe_tests::one_no_route_attempt_among_local_denials_withholds_no_egress` |

Seven of the ten are cases of three `xtask` test binaries that start `cargo` in each case;
together 194 s of the 1303.6 s. Not measured: where the time of one such case goes, and the
rest of the distribution below the ten.

## What changes

1. Measure first: the time of every test of one hosted run by binary, and inside one of the
   seven cases what the `cargo` call costs.
2. What a `cargo` call produces for several cases is made once per run (a setup step before the
   tests, or a cache under `target/` keyed by the digest of its sources), as the collection's
   rule asks of an expensive product.
3. The bound stays 120 s and the thread count stays 12. No shard is added.

## Acceptance

Proposed, for the stakeholder to set: on the hosted runner, `gate-time: test run` of a run on
`main` is at most 90 s. The sum of the test times, which `xtask slowest-tests` prints, is named
in the change with its value before and after.

## Not in this story

The compile of the test binaries (`gate-time: test build`, 586 s to 875 s in these runs): it is
outside the bound.
