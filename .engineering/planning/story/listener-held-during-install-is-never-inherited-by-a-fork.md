---
format: aep.planning-md/3
id: story:listener-held-during-install-is-never-inherited-by-a-fork
kind: story
status: implemented
title: A listener held during install is never inherited by a concurrent fork
relations:
- decomposes: epic:confinement-contract-family
scope:
- confidence: cited
  path: crates/substrate-host/src/egress.rs
revision: 10
transitions:
- {from: "draft", to: "proposed", at: "2026-11-01T07:46:15Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-01T15:13:33Z", actor: "human:timo", revision: 9}
- {from: "active", to: "implemented", at: "2026-11-01T20:46:30Z", actor: "human:timo", revision: 10, decided_on: {"recorded":{"test_result":2,"review_outcome":2}}}
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

## Scope

Derived 2026-11-01 by `story-scoper` at `bf0e0443` (wave D354). Every line is **cited** (read from the story or the tree) or **inferred** (a reading that could be wrong).

- **Primary surface:** `crates/substrate-host/src/egress.rs` — cited (the hold is `install_all` `:512-530`, listeners held `:519-528`)
- **Files:** `crates/substrate-host/src/egress.rs:437-457` (`Listener`, `release`, shutdown on drop), `:512-562` (`install_all`, `install_one`), `:904-954` (`spawn_forwarder`), `:965-1012` (`forwarder_body` and relay fork) — cited
- **Tests:** `crates/substrate-host/src/egress.rs:3517-3553` (failing case, assert at `:3548`), `:3340-3413` (`ForkingNeighbour`), `:3419-3456` (its subprocess-isolated precedent) — cited
- **Symbols:** `install_all`, `install_one`, `Listener::release`, `spawn_forwarder`, `keep_only` (`:1535`), `ForkingNeighbour` — cited
- **Raw fork sites in `crates/substrate-host/src` (9):** production `egress.rs:606` (helper), `:815` (reach probe), `:931` (forwarder), `:996` (relay), `host_process/mod.rs:1593` (`join_from_forked_child`); test-only `egress.rs:1921`, `:2732` (drain), `:3363` (`ForkingNeighbour`), `seccomp.rs:1007` — cited
- **Narrowing:** `keep_only` runs first in the helper (`:681`), reach probe (`:822`) and forwarder (`:976`); the relay closes its listener (`:998`) — cited
- **Long-lived candidate:** test-only `drain` (`:2732`) forks a child with no `keep_only` that reads until its connection ends; the only fork found that can outlive an install — inferred
- **Also likely:** `crates/substrate-host/src/host_process/mod.rs` (the one production fork outside `egress.rs`), `crates/substrate-host/src/process.rs` (sole production caller of `install_all`, `:1301`) — inferred, only if the fix guards every daemon fork or changes install ordering
- **Documents:** `CHANGELOG.md` `[Unreleased]` — inferred
- **Confidence:** medium — fork inventory cited; causal chain inferred, not run
- **Would collide with:** any unit touching `egress.rs` install, forwarder or test code
