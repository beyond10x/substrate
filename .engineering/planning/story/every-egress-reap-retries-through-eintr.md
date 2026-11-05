---
format: aep.planning-md/3
id: story:every-egress-reap-retries-through-eintr
kind: story
status: active
title: Every egress reap outside the abandoned forwarder retries through EINTR
tags:
- from-finding
relations:
- informed_by: review-result:wave-2026-11-04-substrate-close
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-05T06:18:55Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-11-05T06:18:55Z", actor: "human:timo", revision: 4}
---
# Story: every egress reap outside the abandoned forwarder retries through EINTR

## Outcome

A signal handler installed without `SA_RESTART` that interrupts one of the two remaining production
reaps in the egress aperture leaves neither a zombie nor a wrong answer:

- dropping an `InstalledAperture` reaps its forwarder;
- the startup probe `reaches_from_inside` reads its child's real exit status, so an interrupted
  reap is never read as "reached".

## Observation

The 2026-11-04 wave fixed `abandon_forwarder` (`story:abandoned-forwarder-reaped-through-eintr`).
The wave lead's close read found the same un-retried `waitpid` in two more production places
(origin pre-existing, scope out):

- `Drop for InstalledAperture` (`crates/substrate-host/src/egress.rs:429`): on `EINTR` the
  SIGKILLed forwarder stays a zombie until the daemon exits.
- `reaches_from_inside` (`crates/substrate-host/src/egress.rs:719`): on `EINTR` `status` keeps
  its initial `0`, which `WIFEXITED(0) && WEXITSTATUS(0) == 0` reads as reached. The startup probe
  would then report a working mechanism that it never observed.

Inferred from code. Whether any daemon thread installs such a handler was not measured.

## Design

Retry each `waitpid` on `EINTR` and end on any other error, as `abandon_forwarder` now does
(`a_reap_that_fails_other_than_eintr_ends_at_once`). In the probe, a reap that ends with an error
returns false.

## Acceptance

- A case interrupts the drop's reap with a handled signal, then checks that the forwarder is
  reaped.
- A case interrupts the probe's reap, then checks that the probe answers from the child's real
  status; a child exiting 1 never reads as reached.
