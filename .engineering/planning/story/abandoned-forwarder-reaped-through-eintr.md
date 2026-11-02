---
format: aep.planning-md/3
id: story:abandoned-forwarder-reaped-through-eintr
kind: story
status: proposed
title: An abandoned aperture forwarder is reaped through EINTR
relations:
- informed_by: review-result:wave-2026-11-02-d364-u3-security-pass-1
revision: 2
transitions:
- {from: "draft", to: "proposed", at: "2026-11-02T01:25:02Z", actor: "human:timo", revision: 2}
---
# Story: an abandoned aperture forwarder is reaped through EINTR

## Outcome

A forwarder the egress aperture installer abandons on a refusal is reaped even when a signal
handler installed without `SA_RESTART` interrupts the reap, so no refused install leaves a zombie.

## Observation

Wave D364 security pass 1 on U3 (`review-result:wave-2026-11-02-d364-u3-security-pass-1`, origin
pre-existing, scope out): `abandon_forwarder`'s `waitpid` (`crates/substrate-host/src/egress.rs:919`)
does not retry on `EINTR`. The code was moved into `abandon_forwarder` by D364, not changed. Whether
any daemon thread installs such a handler was not measured.

## Design

Retry the `waitpid` on `EINTR`, as the report wait already does for its poll.

## Acceptance

- A case in its own process installs a handler without `SA_RESTART`, signals the installing thread
  during the reap of a refused install, and finds the forwarder reaped and no child left.
