---
format: aep.planning-md/3
id: review-result:adr-0109-fu15-adversary-pass-1
kind: review-result
status: active
title: 'Adversary pass 1: an egress aperture''s listener answers nothing after a refused install'
relations:
- reviews: story:egress-third-install-case-holds-under-load
revision: 1
---
# Adversary pass 1: an egress aperture's listener answers nothing after a refused install

Verdict: one red case on a constructed state. Read at `0a7b78d0` on tree `5677d620`; case on branch
`review/adr-0109-fu-egress-adversary` at `ebee6be6`. Executed 31 to 32, 1 red.

## Findings

- F1 `crates/substrate-host/src/egress.rs:379` (warning, introduced): holding the listener in the
  installing process for the aperture's lifetime turned a dead forwarder's refused connect into a
  handshake that completes into a backlog nobody accepts, so a child hangs until the run's timeout.
  Reached when the forwarder exits on an `accept4` error or is killed inside the run's cgroup; no
  trigger a run can cause on demand was found. Red:
  `a_dead_forwarder_leaves_its_aperture_refusing_rather_than_accepting_into_nothing`.
- F2 `egress.rs:618, 634` (note, pre-existing): `listen_inside` closed a received but rejected
  descriptor without shutdown, before the `Listener` wrap.
- Judgement: production never releases a sandbox after a refused install (`process.rs:1246`), so
  the failure the story fixes is observed only through the test fixture.

## Verified to hold

The mutant without the shutdown turns the forking-neighbour case red; with the fix its result does
not depend on timing; every install failure after the listener exists drops it through the shutdown;
the probe path uses the same type; teardown kills and reaps the forwarder before the shutdown; the
received descriptor is `MSG_CMSG_CLOEXEC`; the installing process never calls `accept`.

## Outcomes

- F1 fixed in `c3119cf6`: `install_all` holds the listeners only across the install loop; a failed
  install shuts each down before killing the forwarders; a complete one releases this process's copy
  without shutdown, so the forwarder holds the last one. The red case passes.
- F2 fixed in `c3119cf6`: the received descriptor is a `Listener` from receipt.
- Host lib `egress::` `ok. 32 passed`; delegated lane EXIT=0 with host lib 335 passed. Merged on
  the follow-up branch as `633106d7`.
