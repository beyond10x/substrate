---
format: aep.planning-md/1
id: review-result:git-workspace-quota-observation-pass-1
kind: review-result
status: active
title: Workspace quota observation independent review
relations:
- reviews: story:git-workspace-quota-lifecycle
revision: 1
---
unit: Substrate workspace quota observation merge on fix/workspace-quota-observation, based on 3979d631667e43e6f8d81251fe259485a6f43c28; final scoped diff SHA-256 48019f791278a890ad8c8a8510c5114b860f9056ef80e5a820c0950713473da8
verdict: nothing found
cases: executed 0→0, red 0 by this reviewer (read-only); retained coordinator output contains the original 1-case red and the corrected 2-case green before the final lease-test strengthening
origin: introduced 0 / pre-existing 0 / undecided 0
wrote-outside-worktree: none
needs-coordinator: full gate is still running; complete validation of the final strengthened test bytes, immutable 0.7.5 release and hosted public-GET accounting proof; release/package/license/planning files are outside this bounded Rust review

1. `git --no-pager diff --stat -- crates/substrate-store/src/workspaces.rs crates/substrate-store/src/tests.rs`

```console
 crates/substrate-store/src/tests.rs      | 119 ++++++++++++++++++++++++++++++-
 crates/substrate-store/src/workspaces.rs |  12 +++-
 2 files changed, 127 insertions(+), 4 deletions(-)
```

The two-file implementation/test diff belongs to the coordinator. My only write is this assigned scratch report; no source or test was changed. The scoped hash above is from `git diff --binary 3979d631667e43e6f8d81251fe259485a6f43c28 -- crates/substrate-store/src/workspaces.rs crates/substrate-store/src/tests.rs`. Concurrent coordinator-owned release metadata and licensing changes are excluded deliberately, so they cannot silently change the scope of this verdict.

2. Cases added and retained runner evidence

None. The task explicitly required read-only review. I did not run a suite, duplicate a build, modify the planning store, invoke an integration tool, or execute a hosted command. I read the complete two-file diff, relevant unchanged callers and helper fixtures, the repository AGENTS.md, accepted storage-accounting design and the story's Hosted observation merge defect acceptance text.

The retained `.scratch/projects-recovery/quota-observation-red.log` contains a real assertion failure in the new case on the original store implementation:

```text
running 1 test
test tests::workspace_observation_merge_refreshes_usage_without_replacing_authority ... FAILED

---- tests::workspace_observation_merge_refreshes_usage_without_replacing_authority stdout ----

thread 'tests::workspace_observation_merge_refreshes_usage_without_replacing_authority' (3533817) panicked at crates/substrate-store/src/tests.rs:2922:5:
assertion `left == right` failed
  left: Some(StorageUsage { limit: StorageLimit { max_bytes: 1048576, max_inodes: 32 }, used_bytes: 4096, used_inodes: 1, observed_at: 2026-08-13T12:00:01Z })
 right: Some(StorageUsage { limit: StorageLimit { max_bytes: 1048576, max_inodes: 32 }, used_bytes: 8192, used_inodes: 32, observed_at: 2026-08-13T12:01:00Z })
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 55 filtered out; finished in 0.00s
```

The retained `.scratch/projects-recovery/quota-observation-green.log` selects both relevant observation tests after the fix:

```text
running 2 tests
test tests::workspace_observation_merge_never_regresses_store_owned_lifecycle ... ok
test tests::workspace_observation_merge_refreshes_usage_without_replacing_authority ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 54 filtered out; finished in 0.00s
```

These are coordinator-produced runner records I read, not executions by this reviewer. The red is the intended behavior failure rather than a compile or fixture-selection error. It proves the store previously returned the creation sample after receiving a later sample. The green demonstrates the initial local merge regression passes; it predates the final lease-test strengthening described below and does not prove those additional assertions executed, kernel enforcement, hosted deployment, or a successful public HTTP observation. The coordinator's full gate was still running when this report was finalized, with final selected-test/Clippy coverage assigned separately if the running gate had already compiled the earlier test bytes.

3. Merge and caller review

The public workspace GET path takes the scoped workspace lock, asks store admission first, and returns the existing resource immediately when it is frozen. For admitted resources it passes the previously admitted resource and stored root to the host, then merges the returned observation. The host preserves the input resource, proves the named filesystem root exists, and obtains quota usage from the quota manager when storage is admitted. The quota manager returns Q_GETQUOTA current byte/inode counters with its own observation timestamp. This is the concrete path that previously computed a fresh sample and then discarded it.

The changed store method retains the existing immediate SQLite transaction and connection mutex. It re-reads the current row by deployment, derived subject and observed resource ID, verifies the stored root still matches, and returns Missing if the row/root is gone. The merge does not upsert a disappeared workspace or cross the subject/root boundary. A lifecycle transition committed while the host observed is therefore checked against the latest durable row, not the earlier request snapshot.

The new assignment is entirely inside the existing guard requiring durable state Ready and no non-active durable lease. Unknown, Destroying and Expired resources, and a Ready resource with a frozen lease, receive no storage or timestamp write from this method. Labels, resource state, lease, identifier and kind remain those read from the store. Admission and lease expiry/renewal policy are unchanged; this observation path never renews or unfreezes a lease.

Both durable and observed storage must exist. The full admitted StorageLimit pair must match, so a driver observation cannot change either byte or inode authority. An unmetered resource cannot acquire a quota through observation, and a missing sample cannot erase an admitted quota. For an accepted sample, only the StorageUsage structure is replaced; equality of its limit has already been required.

Ordering compares the usage sample's own observed_at against the currently durable usage timestamp. Older usage cannot overwrite newer usage, while equal timestamps are accepted as explicitly requested. This check occurs after the current row has been read under the transaction, so it is not comparing against the stale pre-driver request object. Usage counters are not required to be numerically increasing: freeing storage can legitimately reduce them. The existing assignment of the outer workspace observed_at is unchanged; this fix adds ordering for storage observations and does not introduce a general monotonic-clock or outer-timestamp contract.

The caller uses trusted in-process driver output, not a request-provided quota observation. This change adds no HTTP fields, routes, capability activation, quota allocation/release behavior, child execution authority or filesystem mutation. The existing immutable contract bytes are outside the diff.

4. Regression meaning and limits

The new case seeds a stored admitted quota, supplies a later sample with changed bytes and inodes, and asserts both the returned result and a separate durable read contain that complete sample. It deliberately conflicts the labels and verifies store labels/state remain authoritative. The final strengthened version also seeds an Active lease, removes the driver's lease, and requires the complete durable lease to survive alongside the refreshed sample. It then presents an older usage sample, a changed byte limit and an absent sample; each must preserve the updated sample. A separate unmetered resource rejects the same supplied usage. Existing lifecycle coverage is strengthened by adding a real stored sample and a fresher competing sample for Unknown, Destroying and Expired states, requiring the stored sample to survive.

The coordinator addressed the initially reported lease-test evidence gap without changing the production fix. In addition to the Active-lease conflict above, the final Ready/Expiring-lease case now seeds storage, supplies a fresher sample with increased byte usage and an absent driver lease, then requires both the frozen lease and original storage to survive. This specifically challenges the lease-only freeze guard while the workspace state is still Ready. I re-read the complete updated test diff and recomputed the final two-file hash after those changes. The same-limit check compares both fields by type equality, although the test changes only max_bytes. No concurrent process race or wall-clock adjustment was injected during this read-only review.

The known predecessor defect is the subject of the already reproduced and repaired regression, not a newly discovered open finding. Full-suite results, final license/package/version consistency, exact release provenance and real hosted usage after writes remain coordinator work. This report does not claim those checks from the focused store test log.

5. Outside-worktree writes and findings

None. Only `.scratch/projects-recovery/quota-observation-review.md` was written. No source, tests, external scripts, planning data, managed trees, processes, credentials or cluster resources were modified. No costs were exposed.

```findings
[]
```
