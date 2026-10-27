---
format: aep.planning-md/3
id: review-result:release-binaries-security-pass-1
kind: review-result
status: active
title: 'Security review pass 1: prebuilt daemon and CLI binaries per release'
relations:
- reviews: story:release-binaries
revision: 2
---
unit: `release-binaries` commit 124f4b9d (base c2b95f28), with the review cases at 3deec837 on `review/release-binaries-security`, pushed
verdict: NEEDS-CHANGE (F1). Two further warnings (F2, F3) and one note (F4).
cases: executed 33→38, red 1
origin: introduced 4 / pre-existing 0 / undecided 0
wrote-outside-worktree: 6 files under `<scratch>/rb-review/` (run-tests.sh, commit-msg.txt, new-cases.log, new-cases-2.log, suite.log, suite-2.log)
needs-coordinator: none

**Diff stat:** only `xtask/tests/release_workflow.rs` changed (238 lines added). No implementation file was touched.

**Cases added** (all in `xtask/tests/release_workflow.rs`):
- Red: `the_archived_daemon_is_compared_with_the_image_before_the_daemon_tag_is_pushed` (:965) fails with `the archived daemon is compared with the image's at byte 50868, after the daemon tag is pushed at byte 49769`.
- Green guards, which the suite did not check before:
  - `the_checksum_signature_is_verified_against_one_exact_identity_and_issuer` (:981): no `regexp` flag in the workflow, release notes or README, and the issuer is exact.
  - `release_binary_steps_receive_no_token_or_secret` (:1014).
  - `only_the_release_job_holds_packages_and_id_token_write` (:1035).
  - `release_archives_are_deterministic_named_as_documented_and_summed_exactly` (:1109): runs the package step offline twice and gets the same bytes; the entries are exactly the documented ones, owned by 0/0; SHA256SUMS names exactly the two archives.

**Suite run:** `cargo test -p xtask --test release_workflow` gave `FAILED. 37 passed; 1 failed`, EXIT=101.

**Findings:**
- **F1** `.github/workflows/release.yml:961` (blocker, CONFIRMED, introduced): the `cmp` runs after `docker push` at :939. If it ever fails, the write-once daemon tag is already published, and the only way forward is a recovery run, whose branch at :959 skips `cmp`. So this refusal can be bypassed once it has fired. What reaches it: any mismatch between the two builds. Nothing I found produces one today, because the builder layer is cached. Fix: move the `cmp` into the build step, before :939. `target/image-under-test` already exists by :899.
- **F2** `.github/workflows/release.yml:959` (warning, CONFIRMED, introduced): on recovery, the archived daemon is a fresh build from the tagged checkout and is not compared with anything, although `target/image-under-test` holds the reused, signed image's daemon at that point (:927). The binaries do come from the tagged source (checkout at `SOURCE_SHA`, with the tag-moved check). But the release notes say the archive is exported from the same builder stage as the images, and since the build is not reproducible, a recovery run ships a different daemon from the one in the signed image. Fix: in recovery, archive the image's daemon.
- **F3** `.github/workflows/release.yml:794` (warning, CONFIRMED by reading, introduced): a recovery run for a tag cut before this change fails here, because that tag's Dockerfile has no `release-binaries` stage. It fails closed, but such a tag (for example 0.13.0, which is not tagged yet) can no longer be recovered.
- **F4** `README.md:205` (note, CONFIRMED, introduced): the documented identity is `@refs/tags/<version>`, but a recovery-dispatched release is signed as `@refs/heads/main`. The release notes print the actual identity, so they are right. The README section for images has the same pattern.

**What holds:**
- Sign-blob and verify-blob both run before `gh release create`; no step after them is conditional except the summary.
- Identity and issuer are exact.
- SHA256SUMS covers exactly the two archives.
- No token or build argument reaches the new steps; checkout uses `persist-credentials: false`; `.dockerignore` excludes `.git/` and `target/`.
- Permissions are unchanged.
- `ubuntu:22.04@sha256:e0f8…` resolves to an index with an amd64 image that has glibc 2.35, `libssl.so.3` and `libz.so.1`.
- `b10x-substrate --help` exits 0 (`clap` `error.exit()`).
- The `release-binaries` export and the daemon image build should share the cached builder layer, so the added time is about one CLI compile plus an Ubuntu pull. Inferred from the Dockerfile; no CI run seen.

```findings
- {file: .github/workflows/release.yml, line: 961, category: mutant, severity: blocker, verdict: NEEDS-CHANGE, origin: introduced, message: "archive/image cmp runs after the write-once daemon push, and the recovery path then skips it"}
- {file: .github/workflows/release.yml, line: 959, category: judgement, severity: warning, verdict: CONFIRMED, origin: introduced, message: "recovery archives a fresh non-reproducible daemon build instead of the reused signed image's daemon"}
- {file: .github/workflows/release.yml, line: 794, category: contract-drift, severity: warning, verdict: CONFIRMED, origin: introduced, message: "recovery of a tag whose Dockerfile lacks the release-binaries stage now fails"}
- {file: README.md, line: 205, category: contract-drift, severity: note, verdict: CONFIRMED, origin: introduced, message: "documented verify-blob identity is wrong for a recovery-dispatched release signed at refs/heads/main"}
```
