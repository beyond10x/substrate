---
format: aep.planning-md/3
id: story:long-lived-session-output-bound
kind: story
status: draft
title: A long-lived session has an output bound it can run under
summary: 'Design first: a PTY or unrecorded pipe session that runs a service for up to 24 hours is not ended by a 1 MiB total output bound.'
owner: substrate
tags:
- pty
- sessions
relations:
- decomposes: epic:byte-plane-completion
- informed_by: story:output-keeps-tail-within-bound
revision: 1
---
# Story: A long-lived session has an output bound it can run under

## Outcome

A client starts a PTY session (or an unrecorded pipe session) that runs a service for as long as
the run bound allows, 24 hours, and the session does not end because the service wrote more than
1 MiB in total. What the session keeps, and where a client reads it, is decided in a design first
and is stated on the wire, as the current head bound is.

## Why

- A PTY session ends when its output reaches the client's declared `output_bytes`; it is not
  truncated (docs/design/13-pty-sessions.md:76-86; the cost is restated at :198-199). The host
  kills the cgroup on that bound (crates/substrate-host/src/process.rs:3458-3467) and records
  `session.output-limit` on the exec (crates/substrate-host/src/process.rs:3406-3415).
- The declared bound may not exceed the host's `output_limit_bytes`
  (crates/substrate-host/src/process.rs:2534-2537), whose default is `MAX_IO_BYTES`
  (crates/substrate-host/src/lib.rs:472), 1 MiB (crates/substrate-wire/src/lib.rs:22). The same
  check admits `timeout_ms` up to 86 400 000 ms (crates/substrate-host/src/process.rs:2535).
- The bound counts every byte from the start: the terminal stream is drained with
  `Retention::Head` and the same bound flag (crates/substrate-host/src/process.rs:3007-3021), and an
  unrecorded pipe session shares that flag (crates/substrate-host/src/process.rs:3031-3039).
- So a service that writes a few kilobytes a minute ends within hours of a 24-hour run. A
  consumer works around it today by redirecting the service's output to a file in a writable root
  itself, so the session's own stream carries nothing it can read.

## Acceptance

- **A design first** (invariant 8), amending design 13 or as a new design. It weighs at least:
  - a rolling bound: the session keeps the newest `output_bytes` (`Retention::Tail`, crates/substrate-host/src/output_retention.rs:23-24,
    from `story:output-keeps-tail-within-bound`) and ends only on the run's other bounds,
    with a statement on the wire that the head was dropped;
  - output to a file in a declared writable root, written by the daemon, with its own size bound;
  - what a client that attaches late, or re-attaches, reads in each case, since a terminal stream
    has no per-stream offset (docs/design/13-pty-sessions.md:79-80).
  It names the wire members, the capability fact, the ledger record, and whether a bundle bump is
  needed.
- The decided mode is opt-in per session start. Without it, a start's bytes, hash and behaviour
  are unchanged: the session still ends at its bound with `session.output-limit`.
- A daemon case starts a PTY session in the decided mode with `output_bytes` of 4 KiB, writes
  64 KiB, and observes the session still running. The client then reads the kept output as the
  design states and stops the session, and its exec carries no `session.output-limit`.
- The SDK's `pty_session` and `pipe_session` builders carry the mode, and a public-SDK test runs
  the same case.

## Boundaries

- No change to recorded pipe sessions, whose truncation is already reported (`stdout_truncated`,
  docs/design/13-pty-sessions.md:76-78).
- The 24-hour run bound and `MAX_IO_BYTES` for a single read stay as they are.
- The SDK's builder for writable roots is `story:sdk-writable-roots`.
