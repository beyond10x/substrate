---
format: aep.planning-md/3
id: story:aperture-install-sees-forwarder-death-at-once
kind: story
status: active
title: An aperture install sees a forwarder's death at once, whatever another thread forks
relations:
- informed_by: review-result:wave-2026-11-01-d354-u3-adversary-pass-1
scope:
- confidence: cited
  path: crates/substrate-host/src/egress.rs
- confidence: inferred
  path: crates/substrate-host/src/host_process/mod.rs
- confidence: inferred
  path: crates/substrate-host/src/host_process/pidfd.rs
revision: 6
transitions:
- {from: "draft", to: "proposed", at: "2026-11-01T20:34:24Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-01T23:08:45Z", actor: "human:timo", revision: 6}
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

## Scope

Derived 2026-11-02 by `story-scoper` (wave D364). Every line is **cited** (read from the story or the tree) or **inferred** (a reading that could be wrong).

- **Primary surface:** `crates/substrate-host` (`egress.rs` only), cited. The story's three line numbers all resolve in this file.
- **Files:** `crates/substrate-host/src/egress.rs`, cited. `spawn_forwarder` at 802; `handback_pair()` call at 833; `drop(child_end)` at 868; `receive_report(parent_end)` call at 869.
- **Symbols:** `spawn_forwarder`, `receive_report` (def 886), `handback_pair` (def 1492), cited.
- **Pidfd helper:** none in `egress.rs`, cited. `host_process/pidfd.rs` has `open(pid)` and `exited(fd)`, both `pub(super)` inside `mod pidfd;` (`host_process/mod.rs:25`), so `egress` cannot call them as they are, cited.
- **Test file (new case):** `egress.rs`, module `tests` → `observed_inside` → `d354_adv` (3754). The nearest case is `a_forwarder_that_dies_before_its_report_is_refused_at_once_and_reaped` (3946), using `DownNamespace` (3857) and `install_killing_the_forwarder` (3903), asserting under 500 ms, cited. The new case goes beside it, inferred.
- **Helper run in its own process:** `run_alone` (3776) re-execs the test binary with `--exact --ignored` and an env gate; pattern `listener_holders_helper` (4030), `forking_neighbour_dead_forwarder_helper` (3645), cited.
- **Deterministic window:** no seam exists between `handback_pair` (833) and `drop(child_end)` (868), cited absence. Hitting the window once likely needs a `#[cfg(test)]` hook there, inferred.
- **Also likely:** `host_process/pidfd.rs` and `host_process/mod.rs` (widen visibility to `pub(crate)`), inferred, only if the pidfd helper is reused.
- **Documents:** none, cited.
- **Would collide with:** any unit touching `egress.rs` `spawn_forwarder`/`receive_report` or the `d354_adv` module; any unit touching `host_process/pidfd.rs`.
- **Confidence:** high on location; medium on the design (poll on both descriptors and the test seam are inferred).
- **Safety fact:** the forwarder pid is an unreaped child from fork until `receive_report` returns (868-869), so a pidfd opened right after fork names it.
