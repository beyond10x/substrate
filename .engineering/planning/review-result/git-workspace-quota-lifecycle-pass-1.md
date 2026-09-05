---
format: aep.planning-md/1
id: review-result:git-workspace-quota-lifecycle-pass-1
kind: review-result
status: active
title: Independent review of Git workspace quota lifecycle
relations:
- reviews: story:git-workspace-quota-lifecycle
revision: 1
---
unit: story:git-workspace-quota-lifecycle — dirty Substrate tree based on 7ef5321832dad05a523c3cae0ac2463532df81e9
verdict: nothing found
cases: executed 130→132, red 0
origin: introduced 0 / pre-existing 0 / undecided 0
wrote-outside-worktree: 10 paths — ordinary sccache cache plus nine removed roots in the coordinator-owned quota fixture
needs-coordinator: full workspace/release gate and hosted verification remain coordinator-owned

1. `git --no-pager diff --stat`

```console
 .engineering/planning/journal.jsonl                |   7 +
 .../src/git/materialization_tests.rs               |  25 ++-
 crates/substrate-host/src/lib.rs                   | 190 +++++++++++++++------
 crates/substrate-host/src/quota.rs                 |  24 +++
 4 files changed, 194 insertions(+), 52 deletions(-)
```

This tree was handed off with all tracked changes above, the coordinator's untracked story, and the implementor's untracked `quota_tests.rs`. Those are not adversary edits. The saved handed-off production diff and current production diff compare byte for byte with `cmp`, exit 0. My only change appends 149 lines to `crates/substrate-host/src/git/quota_tests.rs`; all 585 existing lines remain an exact prefix, including existing assertions and ignored fixture requirements. `adversary-only-test.diff` and `adversary-only-test-diff-stat.txt` isolate that addition against the pre-addition scratch copy. No source, planning, version, commit, branch, or deployment changes were made by this pass.

2. Added cases and their first isolated executions

The implementation report supplied the pre-addition count of 130 executed cases: portable host package 125 (110 library plus 15 integration), and five separately selected real-quota cases. I did not execute a pre-addition suite. Both cases were appended before compilation or test execution. No existing case was edited.

`crates/substrate-host/src/git/quota_tests.rs`: `real_quota_checkout_exhaustion_cannot_release_another_live_session` first proves the same 2 MiB compressible checkout succeeds under an 8 MiB quota, then submits it under a 1 MiB quota. The small network pack reaches checkout, which must refuse under the kernel ceiling before any post-checkout scan; the failed tree and its kernel limits must be absent. The separately installed live session retains its own identity and exact kernel accounting, and remains destroyable. Current result: green. This exercises the Git workspace create path and legitimate caller-supplied storage limits; no injected daemon state is required.

Command (exit 0):

```console
docker exec -w /task/crates/substrate-host -e SUBSTRATE_TEST_QUOTA_ROOT=/quota -e SUBSTRATE_TEST_PROJECT_QUOTA_IDS=200000-200511 -e TMPDIR=/proc/self/cwd/../../.scratch/projects-recovery projects-recovery-quota-lab-20260905 setpriv --reuid=1000 --regid=1000 --clear-groups --inh-caps=+sys_admin --ambient-caps=+sys_admin /task/target/release/deps/substrate_host-025853158db6070f git::materialization_tests::quota_tests::real_quota_checkout_exhaustion_cannot_release_another_live_session --ignored --exact --test-threads=1 --nocapture

running 1 test
test git::materialization_tests::quota_tests::real_quota_checkout_exhaustion_cannot_release_another_live_session ... real quota fixture root=/quota/git-quota-pAW91T ids=200000-200511
ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 117 filtered out; finished in 0.61s
```

`crates/substrate-host/src/git/quota_tests.rs`: `real_quota_concurrent_installs_keep_independent_identities_and_release` starts two authorized creates together through one driver, checks distinct project IDs and every installed inode's identity, destroys only the first workspace, and proves the second retains accounting while the next create reuses the released first identity. Both remaining trees and kernel limits are removed at the end. Current result: green. This is two ordinary distinct workspace operations sharing the host allocator, not duplicate dispatch or user-selected internal root collision.

Command (exit 0):

```console
docker exec -w /task/crates/substrate-host -e SUBSTRATE_TEST_QUOTA_ROOT=/quota -e SUBSTRATE_TEST_PROJECT_QUOTA_IDS=200000-200511 -e TMPDIR=/proc/self/cwd/../../.scratch/projects-recovery projects-recovery-quota-lab-20260905 setpriv --reuid=1000 --regid=1000 --clear-groups --inh-caps=+sys_admin --ambient-caps=+sys_admin /task/target/release/deps/substrate_host-025853158db6070f git::materialization_tests::quota_tests::real_quota_concurrent_installs_keep_independent_identities_and_release --ignored --exact --test-threads=1 --nocapture

running 1 test
test git::materialization_tests::quota_tests::real_quota_concurrent_installs_keep_independent_identities_and_release ... real quota fixture root=/quota/git-quota-PPhgVK ids=200000-200511
ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 117 filtered out; finished in 1.06s
```

The pre-execution build first failed in sccache with `path must be shorter than SUN_LEN` (exit 101, `adversary-quota-build.log`); a short `/proc/self/cwd` compile TMPDIR then hit `Timed out waiting for server startup` (exit 101, `adversary-quota-build-short-path.log`). Neither failure compiled or selected a case. A command-local `RUSTC_WRAPPER=` disabled that compiler cache wrapper without changing source or compiler options. The release library then compiled successfully in 14.08s (exit 0, `adversary-quota-build-no-wrapper.log`). Complete original error output is retained in those logs. There was no test assertion failure, test mutation, or missed zero-case selection to hide.

Parts 1 and 2 were persisted after these isolated executions and before any suite execution. The complete package and actual enforced-quota lane follow in part 3.

3. Suite executions after part 2 existed

The commands below ran only after both new cases had been independently selected and their first outputs had been persisted above. The enforced fixture lane and portable package were independent executions and ran concurrently, each internally serial. The portable run does not prove kernel enforcement: its seven real-quota cases are explicitly ignored there. Actual enforcement is exercised by the separately selected lane on `/quota`, where each fixture first proves project inheritance and byte/inode EDQUOT refusal before running the case.

Enforced ext4 fixture lane, exit 0:

```console
docker exec -w /task/crates/substrate-host -e SUBSTRATE_TEST_QUOTA_ROOT=/quota -e SUBSTRATE_TEST_PROJECT_QUOTA_IDS=200000-200511 -e TMPDIR=/proc/self/cwd/../../.scratch/projects-recovery projects-recovery-quota-lab-20260905 setpriv --reuid=1000 --regid=1000 --clear-groups --inh-caps=+sys_admin --ambient-caps=+sys_admin /task/target/release/deps/substrate_host-025853158db6070f git::materialization_tests::quota_tests::real_quota_ --ignored --test-threads=1 --nocapture

running 7 tests
test git::materialization_tests::quota_tests::real_quota_byte_and_inode_limits_enforce_later_workspace_mutations ... real quota fixture root=/quota/git-quota-IpKw7y ids=200000-200511
ok
test git::materialization_tests::quota_tests::real_quota_cancelled_fetch_releases_staging_after_the_worker_stops ... real quota fixture root=/quota/git-quota-timppB ids=200000-200511
ok
test git::materialization_tests::quota_tests::real_quota_checkout_exhaustion_cannot_release_another_live_session ... real quota fixture root=/quota/git-quota-snD7tC ids=200000-200511
ok
test git::materialization_tests::quota_tests::real_quota_concurrent_installs_keep_independent_identities_and_release ... real quota fixture root=/quota/git-quota-OS6XWX ids=200000-200511
ok
test git::materialization_tests::quota_tests::real_quota_failed_fetch_removes_staging_and_releases_its_identity ... real quota fixture root=/quota/git-quota-QEaGt9 ids=200000-200511
ok
test git::materialization_tests::quota_tests::real_quota_install_conflict_preserves_the_other_tree_and_releases_staging ... real quota fixture root=/quota/git-quota-MDONUr ids=200000-200511
ok
test git::materialization_tests::quota_tests::real_quota_precedes_git_writes_and_survives_install_restart_destroy ... real quota fixture root=/quota/git-quota-CwKYAS ids=200000-200511
ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 111 filtered out; finished in 3.89s

```

Portable host package, exit 0. Environment is command-local: no compiler cache wrapper, two Cargo jobs, incremental compilation disabled, build TMPDIR inside assigned scratch, and test runner TMPDIR using the short absolute alias to that same scratch directory. No CARGO_TARGET_DIR override; build artifacts remain in this worktree.

```console
RUSTC_WRAPPER= CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 TMPDIR="$PWD/.scratch/projects-recovery" CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_RUNNER='env TMPDIR=/proc/self/cwd/../../.scratch/projects-recovery' cargo test -p b10x-substrate-host --release --locked -- --nocapture --test-threads=1
    Finished `release` profile [optimized] target(s) in 0.16s
     Running unittests src/lib.rs (target/release/deps/substrate_host-025853158db6070f)

running 118 tests
test egress::tests::a_ceiling_in_a_request_is_refused_by_name ... ok
test egress::tests::a_declared_ceiling_stops_the_relay ... ok
test egress::tests::a_forced_bind_failure_names_its_stage_and_errno ... ok
test egress::tests::a_zero_ceiling_binds_the_relay_and_the_parent_alike ... ok
test egress::tests::an_aperture_without_a_ceiling_passes_the_same_traffic ... ok
test egress::tests::aperture_outside_operator_declaration_is_unserved ... ok
test egress::tests::applied_aperture_is_observed ... ok
test egress::tests::declared_aperture_is_reachable ... ok
test egress::tests::egress_defaults_to_none ... ok
test egress::tests::helper_failure_handback_preserves_stage_and_errno ... ok
test egress::tests::helper_handback_preserves_record_boundaries ... ok
test egress::tests::the_apertures_fact_needs_the_mechanism ... ok
test egress::tests::the_apertures_fact_publishes_the_declared_ceiling ... ok
test egress::tests::the_generated_resolution_names_only_the_declared_host ... ok
test egress::tests::the_mechanism_is_proven_in_a_throwaway_sandbox ... ok
test egress::tests::the_sandbox_pid_comes_from_the_info_report ... ok
test egress::tests::the_throwaway_sandbox_these_cases_open_carries_the_user_namespace_posture ... ok
test egress::tests::undeclared_destination_is_unreachable_and_named ... ok
test fs::tests::a_directory_the_operator_already_owns_is_served_under_its_own_name ... ok
test fs::tests::a_file_growing_after_metadata_is_still_read_under_the_complete_file_bound ... ok
test fs::tests::a_hyphenated_project_directory_name_is_a_legal_workspace_root ... ok
test fs::tests::a_root_name_that_is_not_a_single_path_component_is_still_refused ... ok
test fs::tests::atomic_replacement_preserves_the_existing_executable_class ... ok
test fs::tests::create_workspace_still_mints_and_accepts_the_server_prefixed_identity ... ok
test fs::tests::destroy_batches_progress_beyond_former_depth_and_item_caps ... ok
test fs::tests::destroy_removes_fifo_non_utf8_and_deep_entries_without_following ... ok
test fs::tests::guarded_file_and_tree_apis_never_expose_git_control_data ... ok
test fs::tests::guarded_io_refuses_escape_and_observes_atomic_content ... ok
test git::materialization_tests::ambient_git_configuration_cannot_change_fetch_authority_or_transport ... ok
test git::materialization_tests::cancellation_prevents_network_dispatch ... ok
test git::materialization_tests::duplicate_branch_advertisements_are_refused_before_pack_request ... ok
test git::materialization_tests::external_connectors_proxy_v2_fixture ... ignored, requires an external test-only Connectors TLS proxy fixture
test git::materialization_tests::legacy_handshake_is_refused_before_ref_listing_or_pack_request ... ok
test git::materialization_tests::moved_commit_is_refused_before_pack_request ... ok
test git::materialization_tests::quota_tests::absent_quota_is_refused_before_any_git_network_request ... ok
test git::materialization_tests::quota_tests::omitted_storage_keeps_the_existing_git_workspace_contract ... ok
test git::materialization_tests::quota_tests::real_quota_byte_and_inode_limits_enforce_later_workspace_mutations ... ignored, requires the explicitly delegated ext4 project-quota fixture
test git::materialization_tests::quota_tests::real_quota_cancelled_fetch_releases_staging_after_the_worker_stops ... ignored, requires the explicitly delegated ext4 project-quota fixture
test git::materialization_tests::quota_tests::real_quota_checkout_exhaustion_cannot_release_another_live_session ... ignored, requires the explicitly delegated ext4 project-quota fixture
test git::materialization_tests::quota_tests::real_quota_concurrent_installs_keep_independent_identities_and_release ... ignored, requires the explicitly delegated ext4 project-quota fixture
test git::materialization_tests::quota_tests::real_quota_failed_fetch_removes_staging_and_releases_its_identity ... ignored, requires the explicitly delegated ext4 project-quota fixture
test git::materialization_tests::quota_tests::real_quota_install_conflict_preserves_the_other_tree_and_releases_staging ... ignored, requires the explicitly delegated ext4 project-quota fixture
test git::materialization_tests::quota_tests::real_quota_precedes_git_writes_and_survives_install_restart_destroy ... ignored, requires the explicitly delegated ext4 project-quota fixture
test git::materialization_tests::redirects_and_untrusted_tls_are_refused ... ok
test git::materialization_tests::transfer_limit_aborts_real_v2_stream_without_disclosing_authority ... ok
test git::materialization_tests::truncated_pack_never_reaches_checkout ... ok
test git::materialization_tests::v2_materializes_fifty_commits_over_one_tls_connection_and_restricts_refs ... Git fixture: refs=10000 legacy_discovery_bytes=690429 v2_discovery_bytes=214 materialize_ms=170 installed_bytes=68817 installed_inodes=28 tls_connections=1
v2 ls-refs=0014command=ls-refs
0017object-format=sha1
001bagent=git/oxide-0.87.1
0001000csymrefs
0009peel
000bunborn
002bref-prefix refs/heads/provider-default
0000 v2 fetch=0012command=fetch
0017object-format=sha1
001bagent=git/oxide-0.87.1
0001000ethin-pack
000eofs-delta
000edeepen 50
0032want 98ffe015404b2c7552ca79fceedbb99d22c76edd
0009done
0000
ok
test git::network::tests::cancellation_and_deadline_stop_reads_with_redacted_stage_errors ... ok
test git::network::tests::metadata_ceiling_applies_even_when_the_aggregate_budget_is_larger ... ok
test git::network::tests::shared_budget_charges_mixed_reads_once_and_refuses_the_first_extra_byte ... ok
test git::tests::baseline_files_and_changes_are_read_against_host_private_commit_metadata ... ok
test git::tests::dropping_the_async_cancellation_guard_prevents_later_sync_install_stages ... ok
test git::tests::exact_provider_branch_commit_is_verified_and_checked_out_detached ... ok
test git::tests::materialize_errors_do_not_disclose_transient_authority ... ok
test git::tests::startup_reconciliation_removes_only_git_staging_and_orphan_baselines ... ok
test git::tests::synchronization_accounts_nested_git_metadata_and_never_follows_symlinks ... ok
test git::tests::usage_measurement_refuses_byte_and_inode_overflow ... ok
test probe::tests::a_backend_that_cannot_disable_nested_user_namespaces_withholds_the_exec_floor ... ok
test probe::tests::a_child_holding_an_extra_descriptor_withholds_the_fact ... ok
test probe::tests::a_sealed_descriptor_crosses_the_configured_backend ... ok
test probe::tests::a_short_seal_word_withholds_the_fact ... ok
test probe::tests::backend_binding_detects_binary_replacement_with_unchanged_paths ... ok
test probe::tests::declaring_a_slot_moves_the_snapshot_and_rotating_one_does_not ... ok
test probe::tests::every_probe_sandbox_carries_the_user_namespace_posture ... ok
test probe::tests::every_refused_family_is_measured_here_or_recorded_as_instrument_less ... ok
test probe::tests::sessions_pty_is_absent_until_a_probe_proved_a_terminal ... ok
test probe::tests::snapshot_identity_binds_configuration_generation ... ok
test probe::tests::the_confinement_floor_probe_asks_for_and_asserts_a_non_nestable_user_namespace ... ok
test probe::tests::the_published_pty_fact_agrees_with_the_probed_mechanism ... ok
test probe::tests::the_slot_fact_is_names_only_and_needs_every_proof ... ok
test process::tests::a_confined_exec_cannot_autoload_host_kernel_modules_through_af_alg ... ok
test process::tests::a_confined_process_cannot_nest_a_user_namespace ... ok
test process::tests::a_confined_process_cannot_open_an_af_vsock_socket ... ok
test process::tests::an_exec_that_crosses_its_memory_bound_leaves_no_process_in_its_cgroup ... ok
test process::tests::an_oom_is_named_on_the_observation_without_measurements ... ok
test process::tests::capsule_materialization_verifies_bytes_and_cleans_private_directory ... ok
test process::tests::cumulative_cpu_usage_is_read_from_the_exec_cgroup ... ok
test process::tests::every_probe_sandbox_carries_the_user_namespace_posture ... ok
test process::tests::expired_exec_is_terminal_for_exact_acknowledgement ... ok
test process::tests::live_drain_preserves_stream_and_bounded_capture ... ok
test process::tests::narrower_workspace_access_never_degrades_when_the_fact_is_absent ... ok
test process::tests::no_socket_family_opens_inside_a_confined_exec_without_a_recorded_decision ... ok
test process::tests::post_spawn_failure_with_failed_cgroup_cleanup_is_outcome_unknown ... ok
test process::tests::post_spawn_failure_with_proven_empty_cgroup_is_contained_absent ... ok
test process::tests::pty_session_kills_tree_on_attachment_loss ... ok
test process::tests::pty_session_refused_without_confinement ... ok
test process::tests::raw_pipe_refuses_when_hard_confinement_is_unavailable ... ok
test process::tests::saturating_a_live_queue_is_terminal_without_falsifying_durable_truncation ... ok
test process::tests::scoped_workspace_access_refuses_absence_files_and_symlinks ... ok
test process::tests::secret_shaped_environment_names_are_never_admitted ... ok
test process::tests::stale_running_acknowledgement_cannot_discard_newer_terminal_observation ... ok
test process::tests::stale_snapshot_refuses_before_backend_access ... ok
test process::tests::startup_reconciles_only_private_stale_capsule_directories ... ok
test process::tests::terminal_notify_between_state_check_and_wait_is_not_lost ... ok
test process::tests::the_exec_argv_carries_the_user_namespace_posture ... ok
test process::tests::the_user_namespace_posture_is_written_in_exactly_one_place ... ok
test pty::tests::a_confined_terminal_hangs_up_when_the_master_closes ... ok
test pty::tests::pty_resize_is_applied_and_observed ... ok
test pty::tests::the_controlling_terminal_is_taken_inside_the_sandbox ... ok
test quota::tests::facts_publish_the_exact_contract_bounds ... ok
test quota::tests::no_delegated_range_proves_no_quota ... ok
test seccomp::tests::every_surveyed_row_answers_both_questions_and_the_filter_is_built_from_the_same_list ... ok
test seccomp::tests::every_surveyed_socket_family_answers_the_way_the_profile_recorded_it ... ok
test seccomp::tests::no_denied_family_can_be_reached_through_the_x32_socket_syscall ... ok
test seccomp::tests::unix_datagram_socketpair_cannot_be_repurposed_for_host_ipc ... ok
test seccomp::tests::unix_stream_socketpair_remains_available_for_process_local_ipc ... ok
test seccomp::tests::x32_socket_syscall_cannot_bypass_the_unix_socket_refusal ... ok
test secrets::tests::daemon_closes_its_copy_after_spawn ... ok
test secrets::tests::ledger_request_hash_covers_slot_names_only ... ok
test secrets::tests::secret_slot_memfd_is_sealed ... ok
test secrets::tests::secret_slot_refused_when_sealing_unavailable ... ok
test secrets::tests::secret_slot_value_absent_from_argv_env_events_and_ledger ... ok
test secrets::tests::slot_descriptors_are_bounded_and_distinct ... ok
test secrets::tests::the_child_reports_its_declared_descriptor ... ok
test secrets::tests::the_retained_set_reduces_to_stdio_and_the_barrier ... ok
test tests::destroy_driver_call_is_one_batch_then_eventually_absent ... ok
test tests::dropped_blocking_join_keeps_one_process_local_destroy_owner ... ok
test tests::git_source_binding_requires_and_preserves_a_path_segment_boundary ... ok

test result: ok. 110 passed; 0 failed; 8 ignored; 0 measured; 0 filtered out; finished in 36.65s

     Running tests/alg_family_host_state.rs (target/release/deps/alg_family_host_state-94ef11f89963a914)

running 1 test
test a_confined_process_cannot_autoload_a_host_kernel_module_via_af_alg ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/pty_input_after_the_child_ended.rs (target/release/deps/pty_input_after_the_child_ended-a6e63b9483bd83cb)

running 1 test
test input_after_the_child_ended_is_refused_rather_than_reported_delivered ... absent: no delegated cgroup root or no bubblewrap
ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/pty_output_bound_transcript.rs (target/release/deps/pty_output_bound_transcript-747a37bb205e82ac)

running 1 test
test the_durable_terminal_transcript_carries_no_raw_pipe_truncation_statement ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/pty_port_after_the_child_ended.rs (target/release/deps/pty_port_after_the_child_ended-d89b9ec451444522)

running 1 test
test every_port_method_states_its_own_answer_for_a_finished_pty_session ... absent: no delegated cgroup root or no bubblewrap
ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/pty_refusal_order.rs (target/release/deps/pty_refusal_order-a740a1fd20d9d5e1)

running 1 test
test the_absent_pty_fact_outranks_a_missing_window_at_the_driver_port ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

     Running tests/pty_session_acceptance.rs (target/release/deps/pty_session_acceptance-ec71961afd3faa04)

running 6 tests
test a_pty_session_child_has_a_controlling_terminal ... ok
test a_pty_session_resize_delivers_sigwinch_to_the_child ... ok
test a_pty_session_that_crosses_its_output_bound_on_the_last_write_still_names_the_bound ... ok
test a_resize_after_the_child_exited_is_refused_rather_than_reported_applied ... ok
test pty_session_echoes_bytes_and_the_child_observes_a_resize ... ok
test the_driver_port_refuses_a_resize_outside_the_declared_cell_bounds ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/pty_start_refusal_ranking.rs (target/release/deps/pty_start_refusal_ranking-7353a98d00927b84)

running 1 test
test the_overlapping_checks_rank_the_same_at_the_driver_port ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s

     Running tests/qrtr_family_confinement.rs (target/release/deps/qrtr_family_confinement-d7e986fa353f3ace)

running 1 test
test a_confined_process_cannot_open_an_af_qipcrtr_socket ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running tests/user_namespace_floor_documents.rs (target/release/deps/user_namespace_floor_documents-6282e1700467df72)

running 2 tests
test every_document_the_floor_is_named_in_states_the_no_nested_user_namespace_clause ... ok
test the_deployment_list_names_the_backend_options_exec_now_requires ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

   Doc-tests substrate_host

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

```

Executed counts: portable host package 125→125; enforced quota lane 5→7; total distinct executed cases in the assigned scope 130→132, red 0. The portable library now declares 118 cases, of which 110 ran and 8 were ignored (seven actual-quota cases run above, plus the unchanged external Connectors TLS fixture). Some existing delegated execution cases return early when their fixture is absent, as the output states; this report does not claim they proved the hosted execution profile.

Formatting and strict lint commands, both exit 0:

```console
cargo fmt --all --check
```

Formatting output was empty.

```console
RUSTC_WRAPPER= CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 TMPDIR="$PWD/.scratch/projects-recovery" cargo clippy -p b10x-substrate-host --release --all-targets --locked -- -D warnings
    Checking b10x-substrate-host v0.7.3 (/home/timo/.local/state/worktree/trees/b10x/substrate/projects-recovery-substrate-20260905/crates/substrate-host)
    Finished `release` profile [optimized] target(s) in 1.99s
```

4. Findings

Nothing found in the assigned quota lifecycle change.

This report covers the handed-off production diff based on `7ef5321832dad05a523c3cae0ac2463532df81e9`, with the appended cases described above. The code paths and their callers were read before attacks were selected. No unmeasured theory is promoted to a reachable defect, and no finding is routed as pre-existing without reproduction against a base.

5. Attacks that did not break the change

- Apply quota before the first Git metadata or network request: the original real fixture asserts inherited project identity on staging and initialized Git files.
- Expand a small valid Git pack into a checkout larger than its hard ceiling: the new case refuses during checkout while another installed session keeps its independent quota.
- Allocate and install two legitimate concurrent workspaces: distinct project identities cover both installed trees; selective destruction releases exactly one identity for reuse.
- Track the allocator path through atomic install, then restart and destroy: installed usage matches kernel counters and the original case recovers the identity after reopening the driver.
- Truncated fetch, install conflict, and asynchronous cancellation: selected real cases leave no failed target or staging allocation and do not overwrite a competing tree.
- Byte and inode growth after creation: real kernel EDQUOT is observed, followed by exact usage accounting and release after destruction.
- Omitted or unavailable delegation: portable cases retain the existing optional-storage contract or refuse before Git network traffic, respectively.

6. Outside-worktree writes and handoff

The attempted sccache wrapper used the ordinary compiler cache location `/home/timo/.cache/sccache`; successful compilation then ran with that wrapper disabled. No new external build directory was selected. All build products, test source, probe copies, logs, and this report are within the assigned worktree. The container's `/task` path is the same worktree bind mount.

The actual quota tests wrote these nine temporary roots in the already running, coordinator-owned fixture container. Each was removed by its test's TempDir lifecycle. A read-only absence check over all nine paths returned exit 0 after the suites:

- `projects-recovery-quota-lab-20260905:/quota/git-quota-pAW91T`
- `projects-recovery-quota-lab-20260905:/quota/git-quota-PPhgVK`
- `projects-recovery-quota-lab-20260905:/quota/git-quota-IpKw7y`
- `projects-recovery-quota-lab-20260905:/quota/git-quota-timppB`
- `projects-recovery-quota-lab-20260905:/quota/git-quota-snD7tC`
- `projects-recovery-quota-lab-20260905:/quota/git-quota-OS6XWX`
- `projects-recovery-quota-lab-20260905:/quota/git-quota-QEaGt9`
- `projects-recovery-quota-lab-20260905:/quota/git-quota-MDONUr`
- `projects-recovery-quota-lab-20260905:/quota/git-quota-CwKYAS`

```console
docker exec projects-recovery-quota-lab-20260905 sh -c 'for quota_test_path in "$@"; do test ! -e "$quota_test_path" || exit 1; done' -- /quota/git-quota-pAW91T /quota/git-quota-PPhgVK /quota/git-quota-IpKw7y /quota/git-quota-timppB /quota/git-quota-snD7tC /quota/git-quota-OS6XWX /quota/git-quota-QEaGt9 /quota/git-quota-MDONUr /quota/git-quota-CwKYAS
```

That check printed nothing and exited 0. This pass did not create or retire the container, its backing filesystem, mounts, loop devices, or managed worktree. The fixture and assigned scratch remain available for the coordinator's full gate and cleanup. Source and test files were handed back idle after all tests, formatting, and clippy finished. No costs were exposed by these tools.

7. Machine-readable findings

```findings
[]
```
