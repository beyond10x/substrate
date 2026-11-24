---
format: aep.planning-md/3
id: story:gate-test-run-has-margin-under-its-bound
kind: story
status: draft
title: The gate's test run has margin under its 120 s bound on the hosted runner
revision: 2
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
together 194 s of the 1303.6 s.

## What was measured by binary, 2026-11-24, `main` at `5df5154b3`

Step 1 of *What changes*. Gate run 45467477304, job 138706200827: `gate-time: test run: 98 s`,
`gate-time: test build: 544 s`. The log has no JUnit artifact; the times are the `PASS` lines
of cargo-nextest in the job log, the no-default lane of the host driver included: 3,841 lines
whose times sum to 1,124.8 s. 56 tests ran over 5 s and sum to 484.1 s.

The sum by test binary, largest first:

| seconds | tests | binary |
|---|---|---|
| 219.8 | 739 | `b10x-substrate-host`, library |
| 145.7 | 563 | `xtask::bin/xtask` (88.9 s of it the 59 cases of `bundle::tests`) |
| 137.4 | 205 | `b10x-substrate-kubernetes`, library |
| 78.5 | 14 | `b10x-substrate-daemon::hosted_pool_adversary_pass1` |
| 67.5 | 7 | `xtask::kubernetes_client_fake_fence_adversary` |
| 42.2 | 18 | `b10x-substrate-host::confinement_public_items` |
| 29.1 | | `b10x-substrate-daemon::tls_listener` |
| 27.8 | | `b10x-substrate-daemon::pipe_session` |
| 26.3 each | 1 each | the two `xtask` cases that start `cargo` once |

Inside one `xtask` case that starts `cargo`: `check-packages` on the workspace takes 1.5 s of
wall time on the pool's build node and makes 14 `cargo` calls that sum to 6.5 s; nine of them
are `cargo tree -p <member> -e normal,build,dev --target all -e features`, 0.55 s to 0.82 s
each. A case writes a changed manifest into a copy of the workspace, so the answer of each
call depends on the case and no call's product is shared by two cases. Step 2 of *What
changes* therefore has nothing to make once: the time of these cases is `cargo` resolving a
workspace, and they are left as they are. The `bundle::tests` cases render and compare
bundles and compute for their time.

## What changed

Three changes, the first two in test code only. The seconds are of the pool's build node in
the release profile; the hosted runner's `gate-time: test run` is read from the pull
request's gate run and named there.

1. Twelve cases of the daemon's `hosted_pool_adversary_pass1` and `tls_listener` and the
   SDK's `remote` sent `SIGTERM` to the daemon while their own client streams were open, and
   the daemon then waits up to 5 s for open TLS connections. They close their streams first.
   The idle-timeout case polls the count of open Identity connections every 50 ms where it
   slept 31.5 s. Two `pipe_session` cases waited 5 s for a frame an accepted resize never
   sends; they wait for the first of that frame and the window reaching the driver. Seconds
   on the pool's build node, release profile: `hosted_pool_adversary_pass1` 77.4 to 35.9,
   `tls_listener` 21.0 to 0.9, `pipe_session` 10.2 to 0.13, the SDK case 5.2 to 0.2.
2. Five probe cases of the namespace driver ran 3 to 15 probes one after the other, each
   with a fake API server of its own; they run at the same time, each on a thread and a
   runtime of its own. The stand-in executor of `workspace_tests` holds a lost answer until
   the driver's next call where it slept 1.6 s. The host driver's posture case runs its
   three probes on three threads, and `ForkingNeighbour::join` ends the children it reaps.
   Seconds: the five probe cases 19.4 to 2.8, 8.7 to 2.2, 7.3 to 3.0, 7.0 to 2.7 and 6.8 to
   1.0; the namespace driver's library sum 131.3 to 88.9; the fork case 5.1 to 0.03.
3. A startup probe of the host driver ends when its sandbox has ended: the terminal probe
   behind `sessions.pty` and the egress aperture probe answer that the mechanism is not
   proved once the backend they started is gone. Their deadlines, 10 s and 5 s, are
   unchanged. This one is a change to shipped code and has a line in the changelog. Held by
   `a_sandbox_that_ends_at_once_ends_the_probe_before_its_deadline` in `pty::tests` and in
   `egress::tests`, 0.12 s and 0.16 s. The posture case, which runs the three probes through
   a backend that exits at once: 0.17 s. The host library: 741 of 741.

## Kept, with the reason

Each waits out a bound that shipped code states as a constant, over a real socket. Shortening
one needs the constant to be settable, which is a change to shipped code and its own story:

- `adversary_idle_connection_older_than_the_idle_timeout_is_closed`, 30 s: the daemon's
  Identity idle timeout (`IDENTITY_IDLE_TIMEOUT` in `crates/substrate-daemon/src/hosted.rs`,
  ADR 0040), which no flag sets.
- `readiness_reads_the_snapshot_taken_once_the_first_open_sequence_has_settled`, 10 s: the
  namespace driver's retry period (`RENEW_EVERY` in `crates/substrate-kubernetes/src/lease.rs`).
- One case at the access review bound of 5 s (`ACCESS_REVIEW_BOUND`), three pod cleanup cases
  at the replay limit of 8 s (`REPLAY_LIMIT` in `crates/substrate-kubernetes-client/src/gone.rs`)
  and one `adversary_pod` case at 5 s (`SILENT_TIMING`).
- The probe cases of the namespace driver keep 0.67 s a case on average from `PROBE_POLL`
  (200 ms) and `HEARTBEAT_STILL` (2 s) in `namespace/probe.rs`.
- The cases of `metrics_stream_adversary` assert whole 1 s sample intervals.

Kept because they compute: `confinement_public_items` (0.75 s of base listing in each of 18
cases), two Git create cases, the two exec-gate walks at 65,536 rules, the `xtask` cases
above.

One observation that is the pool's and not this repository's:
`writable_roots::tests::a_changed_root_is_refused_by_name_with_its_code` requires that `/usr`
is not the test user's. It failed in three runs of the host library on the pool's build node
and passed in a fourth and in a run of that one case on unchanged `main`; it passes on the
hosted runner.

## What changes

1. Measure first: the time of every test of one hosted run by binary, and inside one of the
   seven cases what the `cargo` call costs. Done, above.
2. What a `cargo` call produces for several cases is made once per run (a setup step before the
   tests, or a cache under `target/` keyed by the digest of its sources), as the collection's
   rule asks of an expensive product. Measured: no call's product is shared, above.
3. The bound stays 120 s and the thread count stays 12. No shard is added.

## Acceptance

On the hosted runner, `gate-time: test run` of a run on `main` is at most 90 s. The sum of the
test times, which `xtask slowest-tests` prints, is named in the change with its value before
and after.

## Not in this story

The compile of the test binaries (`gate-time: test build`, 544 s to 875 s in these runs): it is
outside the bound.
