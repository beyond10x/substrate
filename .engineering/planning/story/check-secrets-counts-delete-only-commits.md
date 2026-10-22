---
format: aep.planning-md/3
id: story:check-secrets-counts-delete-only-commits
kind: story
status: draft
title: check-secrets accepts a commit that only deletes files
revision: 2
---
## Problem

`cargo xtask check-secrets` (`xtask/src/secrets.rs`) requires the number of commits Gitleaks reports as scanned to equal the reachable commits minus those with no patch (`treeless_commits`: an empty root tree, or a tree identical to every parent's). A commit whose only change deletes files has a patch, so it is counted as expected, but Gitleaks does not report it as scanned.

Observed 2026-10-22 on branch `wave/2026-10-22-writable-roots-u1` (PR #150, closed): commit `dfade77d` only deleted `crates/substrate-daemon/tests/writable_roots_contract_adversary.rs`, and the Full gate refused with `Gitleaks reported Some(552) scanned commits; Git reports 553 with a patch`. The same history without that commit passed. Confirmed in Gitleaks 8.30.1 (the pinned `VERSION` in `xtask/src/secrets.rs`): `sources/git.go:323` skips a file diff with `IsDelete`, and `:329` skips a binary that is not an archive, before the commit reaches `addCommit` (`detect/detect.go:759`), so a commit whose every file diff is a deletion or a non-archive binary is never counted in "N commits scanned".

Consequence: any delete-only commit that reaches `main` makes `check-secrets` fail on every later gate run, and removing it needs a history rewrite.

## Acceptance

- `check-secrets` passes on a history containing a commit that only deletes a file, and still fails when Gitleaks scans fewer commits than carry added content.
- A test builds such a history in a temporary repository and runs the count against it.
- The same holds for a commit that only changes a non-archive binary file.

CI checks out with `fetch-depth: 0` (`.github/workflows/gate.yml:57`), so a delete-only commit on any remote branch, merged or not, turns every gate run red: PR #151 failed with `Gitleaks reported Some(553) scanned commits; Git reports 554 with a patch` while the closed #150 branch still held `dfade77d`.
