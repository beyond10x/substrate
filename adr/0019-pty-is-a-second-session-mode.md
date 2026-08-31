---
status: accepted
date: 2026-08-31
---

# ADR 0019: a PTY is a second session mode

## Context

[ADR 0007](0007-protocol-processes-use-raw-pipe-sessions.md) deliberately served raw-pipe
sessions first and refused to substitute a terminal for pipes. [ADR 0008](0008-pipe-sessions-have-distinct-durable-identity.md)
then made that byte plane a durable `ses_…` resource with one lease, one attachment and one
whole-tree cleanup path. A human-facing interactive process still needs terminal line discipline,
window changes and hangup semantics, but duplicating the resource lifecycle would create two names
for the same authority and two places for its bounds to drift.

The accepted choice must also preserve the route bytes already published as
`/v1/pipe-sessions/*`. Renaming those routes removes verified operations and is not an additive
successor. Adding a parallel PTY family duplicates mode-neutral operations such as start, get,
attach, signal, renew and retire.

## Decision

**PTY is a `SessionMode`, not a new resource kind.** The existing session resource, operation
ledger, lease, attachment limit and `/v1/pipe-sessions/*` route family serve both `pipes` and
`pty`. An omitted mode continues to mean `pipes`; a PTY is returned only when the request
explicitly asks for it. No PTY request may fall back to pipes.

**A PTY channel has a distinct closed frame vocabulary.** Client frames are `input`, `resize` and
`signal`; server frames are `output`, `exit` and `protocol-error`. Output has no stream selector
because a terminal merges stdout and stderr. There is no half-close frame: terminal EOF remains an
input byte governed by the line discipline.

**The initial window is required and every window is bounded.** A PTY request supplies columns and
rows in cells, each in the inclusive range 1–1000. Zero is not treated as unknown and no 80×24
default is manufactured. Resize uses the existing bounded control window; an invalid size is the
named protocol error `session.resize-invalid`. Substrate injects neither `TERM` nor `COLUMNS` /
`LINES` into the cleared environment.

**The terminal stays inside the existing confinement path.** The per-session master never crosses
the daemon boundary. The slave becomes descriptors 0, 1 and 2 only inside the existing
bubblewrap/cgroup/environment path. Bubblewrap's `--new-session` remains part of the isolation
floor; the child acquires its controlling terminal after that `setsid`, rather than weakening the
shared sandbox. Closing an attachment, lease expiry, protocol failure and retirement retain the
same whole-tree cleanup guarantee as pipes.

**Capability is proven and refusal is named.** `sessions.pty` is a backend-identity-bound driver
fact published only after a startup probe allocates a terminal, makes it controlling in a
throwaway sandbox and round-trips a window. A request without that fact is
`unserved`/`session.pty-unserved` at `mode`. Allocation failure before dispatch is
`exhausted`/`session.pty-exhausted`; it is retriable. Supplying a window for pipes or omitting one
for PTY is `refused`/`session.window-invalid`.

**The declared output bound is terminal for PTY.** A terminal has no resumable per-stream offset
and the frame vocabulary has no truncation notice. Reaching the bound therefore cancels the
session and records `exhausted`/`session.output-limit`; it never silently truncates a live
transcript.

**The contract change is bundle 0.10.0.** Bundle 0.9.0 is already released by ADR 0018's v2 route
closure. Its successor names 0.9.0 as predecessor, adds no routes, preserves all 31 routes, and
adds the PTY mode, terminal frames, capability fields and refusal register. Bundles 0.1.0 through
0.9.0 remain byte-immutable.

## Consequences

A human can run an interactive terminal without gaining a second resource or a weaker sandbox.
Existing pipe clients retain their route, default mode and frame semantics. Clients that request a
terminal can distinguish unavailable capability, invalid window, allocation pressure, protocol
failure and output exhaustion without inspecting driver internals.

The route family keeps a historical name that describes its first mode. Changing that identifier
later remains a coordinated migration, not cleanup. The positive confinement proof requires the
delegated lane; hosted CI proves request and schema refusals and reports the delegated lane absent.

The complete reasoning and rejected alternatives are recorded in
[design 13](../docs/design/13-pty-sessions.md).
