---
format: aep.planning-md/3
id: story:confinement-runs-on-a-terminal
kind: story
status: draft
title: The confinement API runs a command on a terminal
summary: ConfinementProfile gains a PTY standard input with a window, a fact and a resize; local and linux prove it, reusing the daemon session path's pty mechanism.
owner: substrate
tags:
- confinement
- pty
relations:
- decomposes: epic:confinement-contract-family
revision: 1
---
# Story: The confinement API runs a command on a terminal

## Outcome

An embedder of `substrate_host::confinement` admits a profile whose standard input and output are
a pseudo-terminal with a window. Through the adapter it then writes input, reads live output,
resizes the window and sees a hangup when the run ends, as the daemon's PTY session already does.
An adapter that cannot prove the mechanism refuses the profile by name.

## Why

- The adapter trait has `write_stdin`, `close_stdin`, `read_output` and `signal`, but no terminal
  and no resize (crates/substrate-host/src/confinement/mod.rs:723-826). `StdinMode` is `Null` or
  `Piped` (crates/substrate-host/src/confinement/profile.rs:110-117), and the facts end at
  `stdin_pipe` and `output_live` (crates/substrate-host/src/confinement/facts.rs:40-41; design 32
  § 4, docs/design/32-adapter-stdin-and-live-output.md:353-354).
- The mechanism already exists in the same crate, but only on the daemon's session path:
  `pty::open`, `resize` and the sandboxed terminal probes (crates/substrate-host/src/pty.rs:92,
  225, 319), and the controlling terminal taken after bubblewrap's `setsid`
  (crates/substrate-host/src/process.rs:2830-2836).
- A consumer that gave up its own service-manager and terminal-multiplexer confinement places
  every run through Substrate. Interactive agent sessions need a terminal, and the in-process API
  is the only Substrate surface a run inside a box can use: the `local` adapter spawns an ordinary
  process (crates/substrate-host/src/confinement/local.rs:1-6), and no daemon socket reaches the
  box (`story:a-run-in-a-box-starts-attachable-pty-sessions-in-its-own-box`).

## Acceptance

- A design (an amendment to design 32 or a new design) comes first (invariant 8). It decides the
  `StdinMode` or profile member for a terminal with its window, a fact (for example `terminal`)
  appended to `Fact` and `FACT_ORDER`, and a `resize` method on `ConfinementAdapter` that defaults
  to a named refusal.
- The `substrate.confinement` domain (spec/confinement/confinement.yaml, the `Fact` enum at
  lines 50-65) gains the fact. `cargo xtask check-ess` regenerates and passes.
- `local` and `linux` prove the fact. On `linux` the proof is the probe the daemon session path
  already uses (crates/substrate-host/src/pty.rs:319). Every other adapter keeps it absent. A
  terminal profile there is refused `confinement.fact-unserved` naming the fact.
- One case per serving adapter: start `sh` on a terminal, write `stty size; echo ok`, read the
  window and `ok` from live output, resize, read the new size, kill the run, observe its end.
- No wire or bundle change (ADR 0044 decision 7: this is substrate-host's API).

## Boundaries

- No attach by a second client and no ledger record: that is the daemon's job, and
  `story:a-run-in-a-box-starts-attachable-pty-sessions-in-its-own-box`.
- No terminal on `oci`, `pod`, `macos` or `systemd` in this story.
