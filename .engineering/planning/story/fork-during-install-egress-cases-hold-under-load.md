---
format: aep.planning-md/3
id: story:fork-during-install-egress-cases-hold-under-load
kind: story
status: draft
title: The fork-during-install egress cases hold under a loaded machine
summary: Two egress cases failed 2 of 7 host-crate runs under load; the neighbour thread had not forked during the install.
owner: substrate
relations:
- informed_by: story:listener-held-during-install-is-never-inherited-by-a-fork
revision: 1
---
# Story: the fork-during-install egress cases hold under a loaded machine

## Observation (2026-11-28)

On branch `wave/2026-11-28-rootfs-unpacker` (no change to `crates/substrate-host/src/egress.rs`),
`cargo nextest run -p b10x-substrate-host --release --locked` failed 2 of 7 runs on a loaded
workstation, each time one case of the fork-during-install group:

- run 1: `a_listener_held_during_install_is_not_inherited_by_a_concurrent_fork`, "the helper failed";
- run 2: `a_failed_third_install_leaves_no_listener_answering_while_another_thread_forks`.

The helper's message in the second was "the neighbour forked nothing": the install ended before the
neighbour thread forked once. The other 5 runs passed; the group alone passed 3 of 3; the crate
without the new image tests passed 2 of 2. One full run took 102.5 s under load and 26 s on a quiet
machine.

Origin not established: the same suite was not run repeatedly at `main` (`a9e3d52`). Inferred: a
timing race in the case, not in the code, made likelier by CPU load from other tests.

## Outcome

Each case of the group either proves the neighbour forked during the install, or retries the
install until it did within a bounded number of attempts, and fails only when the rule is broken,
never because the neighbour thread did not run in time.

## Acceptance

- The group passes 20 of 20 full host-crate runs on a loaded workstation (recorded in this story,
  not a test loop).
- A case whose neighbour did not fork during the install says so and retries, within the 60 s bound.

## Where the requirement comes from

Found by the session that works this repository while building
`story:root-filesystem-from-a-digest-named-archive`. Related: `story:listener-held-during-install-is-never-inherited-by-a-fork`
(implemented).
