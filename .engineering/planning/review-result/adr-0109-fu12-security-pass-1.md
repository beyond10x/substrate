---
format: aep.planning-md/3
id: review-result:adr-0109-fu12-security-pass-1
kind: review-result
status: active
title: 'Security review pass 1: a SIGKILL''d confine run leaves no confine group behind'
relations:
- reviews: story:confine-run-reaps-killed-groups
revision: 1
---
# Security review pass 1: a SIGKILL'd confine run leaves no confine group behind

Verdict: needs change. Read at `0cb24ff5` (diff against `0da49ff2`); cases on branch
`review/adr-0109-fu-host-security` at `89d1e32d` in `crates/b10x-substrate-cli/tests/confine.rs`,
executed 16 to 18, 1 red, in a delegated scope.

## Findings

- F1 `crates/b10x-substrate-cli/src/confine/cgroup.rs:189-205` (warning): `reap` discards every
  failure without a word: an unreadable root, a dead owner's group that still holds a process, and
  a failed removal. Red case: `a_dead_owners_group_left_holding_a_process_is_named_on_stderr`.
- F2 `cgroup.rs:153-158, 198` (note, infeasible): owner liveness is read through this process's
  own `/proc`, so an owner in another pid namespace or hidden by `hidepid` reads as gone; its group
  is removable only while empty, between `create_dir` and the leaf move.
- F3 `cgroup.rs:161-167` (note, infeasible): a 0.12.0-format `confine-<pid>-<nanos>` name parses
  as an owner whose start time never matches; removable only in the same empty window.

## Verified to hold

Nothing is killed (`reap` only calls `remove_dir`, refused by the kernel on a populated group; green
case `a_dead_owners_group_that_still_holds_a_process_is_kept_and_its_process_runs_on`); the 0.12.0
shared-root kill does not return; `substrate-ex_*` and other names are untouched; nothing outside
the root; symlinks and non-directories skipped; no race between listing and removal; no new
privilege.

## Outcomes

- F1 fixed in `53b28582`: one stderr line per group left or not removable and one for an
  unreadable root; exit status unchanged. Under `--output json` the groups go into the verb's one
  document as `left_groups` (`bfdf7873`). Both cases pass; their `executed` lines are required by
  `scripts/delegated-lane.sh`.
- F2 and F3: accepted as notes; the reaper never kills, so the worst case is a failed start of the
  other invocation.
- Merged on the follow-up branch as `979e5c8a`.
