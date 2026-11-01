---
format: aep.planning-md/3
id: story:listener-held-during-install-is-never-inherited-by-a-fork
kind: story
status: proposed
title: A listener held during install is never inherited by a concurrent fork
relations:
- decomposes: epic:confinement-contract-family
scope:
- confidence: inferred
  path: crates/substrate-host/src/egress.rs
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-01T07:46:15Z", actor: "human:timo", revision: 2}
---
# Story: a listener held during install is never inherited by a concurrent fork

## Observation

The D342 full gate at `fad2a122` (2026-11-01, 07:42Z, release profile) failed once in
`substrate-host`: `egress::tests::observed_inside::a_dead_forwarder_leaves_its_aperture_refusing_rather_than_accepting_into_nothing`
(`crates/substrate-host/src/egress.rs:3548`) read `open` where it expects `refused`. The same test
passed 5 times alone and the host lib passed 3 times in full afterwards; no D342 commit touches
`crates/substrate-host`.

Inferred, not verified: `install_all` holds each received listener across the install loop
(`c3119cf6`). Forwarders and relays are made with a raw `fork()` and no `exec`, so `SOCK_CLOEXEC`
does not close what they inherit; a fork in another thread inside that window keeps a copy of the
listener open after the aperture's own forwarder dies, and a connect from inside completes into a
backlog nobody accepts. If so, two concurrent installs in one daemon could do the same.

## Acceptance

- A red case first: while one thread installs an aperture list, another thread forks and keeps its
  child alive; after the first list's forwarder is killed, a connect from inside its aperture is
  refused.
- No process forked by the host outside the aperture's own forwarder holds that aperture's
  listener once its install returns.
- The dead-forwarder test passes 200 times in a row in the host lib run in parallel.
