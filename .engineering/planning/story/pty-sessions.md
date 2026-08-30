---
format: aep.planning-md/1
id: story:pty-sessions
kind: story
status: draft
title: PTY sessions are a distinct session kind with resize
summary: Design 05 section 2 fixes the pty kind and frames; README lists PTY as absent; needs ADR and a successor bundle before code.
owner: substrate
tags:
- daemon
- host
- wire
relations:
- decomposes: epic:byte-plane-completion
revision: 3
---
# Story: PTY sessions are a distinct session kind with resize

## Outcome

A human can attach a terminal to a confined process, resize it, and get the same lease, single
attachment, bounded-output and whole-tree cleanup guarantees the raw-pipe session gives.

## Context

`docs/design/05-streams-sessions-and-endpoints.md` § 2 fixes the `pty` kind and its frames
(input, output, resize, signal, exit, protocol-error) and says a PTY is never substituted for
pipes. `README.md` § *Status* lists PTY as **absent**; `docs/plan/04-direct-byte-plane.md`
§ *Later phase-4 slices* lists it as remaining. Invariant 8 puts the ADR before code; invariant 6
puts the wire change in a successor bundle.

## Acceptance

The delegated lane runs an interactive shell through a `pty` session, echoes bytes, applies a
resize the child observes, and exits on hangup — while the portable lane proves the typed
`unserved` and every `0.4.0` byte is unchanged.

Evidence that satisfies it, in order:

1. A design document in `docs/design/`, proposed, fixing kind, frame set, resize bounds, never a
   substitute for pipes, and the named refusal when the host cannot allocate a pty. It claims no
   ADR number: `adr/` admits `accepted` and `superseded` only (`xtask/src/adrs.rs:12`), so the
   number is assigned at acceptance.
2. The next free successor bundle under `contracts/substrate-wire/` adds the `pty` kind and
   `resize` frame; compatibility block names its predecessor with exact
   `adds_routes`/`preserves_routes`; authored source under `xtask/bundle-source/<version>/` and
   `cargo xtask check-bundle <version>` in `scripts/gate.sh`.
3. `crates/substrate-host`: pty opened with `nix::pty::openpty` inside the existing
   bubblewrap/cgroup/cleared-environment path. Failing-first tests:
   `pty_session_refused_without_confinement`, `pty_resize_is_applied_and_observed` (child reads
   `TIOCGWINSZ`), `pty_session_kills_tree_on_attachment_loss`.
4. Daemon: the single-attachment Unix-WebSocket route family serves `pty`; `resize` outside the
   declared bounds is a protocol error.
5. Capability fact `sessions.pty` appears only after startup verified pty allocation (invariant 4).
6. `STATUS.md`, `README.md` § *Status* and plan 04 § *Later phase-4 slices* updated in the same
   change.

## Open Questions

Bundle number: `0.5.0` if this lands first in the epic; the ADR states the actual number.

## Correction — 2026-08-30

Three claims in the evidence list above had gone stale and are rewritten, not deleted, so the
change is visible:

| Claimed | Actually |
|---|---|
| `adr/0011-pty-sessions-are-a-distinct-session-kind.md` | `0011` is `adr/0011-delegated-context-and-grant-attribution.md`; the number was taken while this story sat in draft |
| `contracts/substrate-wire/0.5.0/` | `0.5.0` is sealed secret slots and `0.6.0` is destination-bound egress; both are frozen (invariant 6) |
| `render-contract-bundle-0.5.0.py`, `check-contract-bundle-0.5.0.py` | the Python renderer/checker pairs stop at `0.4.0`; from `0.5.0` on the check is `cargo xtask check-bundle <version>` (`scripts/gate.sh:24-28`) |

The story pins no numbers now, so it cannot go stale the same way again.
