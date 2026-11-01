---
format: aep.planning-md/3
id: story:aperture-install-sees-forwarder-death-at-once
kind: story
status: proposed
title: An aperture install sees a forwarder's death at once, whatever another thread forks
relations:
- informed_by: review-result:wave-2026-11-01-d354-u3-adversary-pass-1
revision: 2
transitions:
- {from: "draft", to: "proposed", at: "2026-11-01T20:34:24Z", actor: "human:timo", revision: 2}
---
# Story: an aperture install sees a forwarder's death at once, whatever another thread forks

## Outcome

An egress aperture install whose forwarder dies before its report is refused at once even when
another thread of the daemon forked between the report socket's creation and the installer closing
its child end, and keeps that child alive.

## Observation

Wave D354 adversary pass on U3 (`review-result:wave-2026-11-01-d354-u3-adversary-pass-1`, origin
pre-existing): a kept-alive fork between `handback_pair()` (`crates/substrate-host/src/egress.rs:832`)
and `drop(child_end)` (`:868`) holds the report socket's other end, so `receive_report` (`:869`) sees
EOF only when that child exits. Measured with a local 20-round loop: 2 of 20 refusals took 2.0096 s
and 2.0094 s, the neighbour's hold. Only kept-alive test forks were shown to reach it; production
forks exec or close their descriptors first (inferred, not measured). The pre-D354 `listen_inside`
hand-back had the same window.

## Design

Wait on a pidfd of the forwarder beside the report socket, so the forwarder's exit ends the wait
whoever holds the socket's other end.

## Acceptance

- A case kills the forwarder before its report while a neighbour thread's child, forked inside that
  window, stays alive; the install is refused within 500 ms. The case runs once.
