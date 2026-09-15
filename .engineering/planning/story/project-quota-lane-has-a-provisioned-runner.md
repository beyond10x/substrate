---
format: aep.planning-md/1
id: story:project-quota-lane-has-a-provisioned-runner
kind: story
status: draft
title: The project-quota lane runs somewhere, and a release says when it last did
relations:
- informed_by: story:git-workspace-quota-lifecycle
scope:
- confidence: inferred
  path: .github/workflows/release.yml
- confidence: inferred
  path: AGENTS.md
- confidence: inferred
  path: scripts/delegated-lane.sh
- confidence: inferred
  path: xtask/src/quota_lane.rs
- confidence: inferred
  path: xtask/tests/release_workflow.rs
revision: 2
---
# Story: The project-quota lane runs somewhere, and a release says when it last did

## Context

`cargo xtask quota-lane` now runs the seven `real_quota_*` cases in
`crates/substrate-host/src/git/quota_tests.rs` and refuses by name when
`SUBSTRATE_TEST_QUOTA_ROOT` / `SUBSTRATE_TEST_PROJECT_QUOTA_IDS` are absent or the backing mount
carries no project quotas, and `scripts/delegated-lane.sh` closes with
`cargo xtask quota-lane --inventory` so a green delegated lane cannot be read as covering them.
That makes the cases *runnable and honestly reported*. It does not make them *run*.

Two things are still open, and they are the reason kernel project-quota enforcement rests on a
single hand-typed execution dated 2026-09-05
(`review-result:git-workspace-quota-lifecycle-pass-1`, the three `docker exec … --ignored`
commands):

- **Nothing provisions the fixture.** It needs a filesystem mounted with project quotas
  (`prjquota`, or XFS `pquota`) and an exclusive inclusive identity range of at least 128, plus
  `SYS_ADMIN` in the inheritable and ambient sets for `quotactl`. A hosted runner has none of that,
  and a developer host generally does not either — `findmnt -no SOURCE,FSTYPE,OPTIONS /` on the
  machine this story was written on reports `ext4 rw,noatime`, and `/proc/self/mountinfo` carries
  no `prjquota` on any mount.
- **No release condition asserts them.** The `Confinement-lane:` trailer
  (`AGENTS.md`, § *Releases*) names the `DELEGATED_CASES` of
  `crates/substrate-daemon/tests/runtime_vectors.rs`; the real-quota cases are in another crate and
  outside that count, so every release so far shipped project-quota enforcement on the strength of
  one dated manual run.

## Acceptance

A reproducible fixture exists that `cargo xtask quota-lane` runs green — a checked-in image or
loop-device recipe, invoked by one documented command — and a release refuses without a recorded
run of it for the tagged commit, in the same shape and with the same refusals as
`Confinement-lane:`: wrong commit refused, wrong case count refused, non-RFC-3339 timestamp
refused, with the offline assertions in `xtask/tests/release_workflow.rs`.

## Notes

The runner and the release condition are separable: a provisioned fixture with no release condition
is still worth more than today, because the lane can then be run on demand and cited. Doing the
release condition first would be a condition nothing can satisfy.

Do not let a green `cargo xtask quota-lane --inventory` line in a delegated-lane log stand in for a
run. It is a statement that the cases did **not** run; that is its whole purpose.
