---
format: aep.planning-md/3
id: story:a-run-in-a-box-starts-attachable-pty-sessions-in-its-own-box
kind: story
status: draft
title: A run in a box starts attachable PTY sessions in its own box
summary: 'ADR first: a confined run reaches a daemon endpoint scoped to its own workspace and starts PTY sessions there that the owner attaches to; amends ADR 0015.'
owner: substrate
tags:
- pty
- sessions
relations:
- decomposes: epic:byte-plane-completion
- informed_by: story:confined-processes-cannot-nest-user-namespaces
- informed_by: story:confinement-runs-on-a-terminal
revision: 1
---
# Story: A run in a box starts attachable PTY sessions in its own box

## Outcome

A process running in a confined workspace asks the daemon to start a PTY session in that same
workspace, and only there. The session is an ordinary daemon session: recorded on the ledger,
attachable by the workspace's owner through the SDK (`PipeSession::attach`,
crates/b10x-substrate-sdk/src/lib.rs:1476), resizable and fed input. The run gains no authority
over any other workspace, exec or daemon route.

## Why

A consumer runs its runtime in a box (a workspace with a pipe session, through the Rust SDK). That
runtime must start interactive agent sessions in the same box, and every placement must go
through Substrate. On the current tree it cannot:

- No daemon socket reaches a box. ADR 0015 has every confined process run under a seccomp program
  that denies `AF_UNIX` socket creation (adr/0015-declared-host-roots-carry-no-host-ipc.md:17-21).
  `AF_UNIX` is missing from `ALLOWED_SOCKETS` (crates/substrate-host/src/seccomp.rs:150-152). The
  SDK reaches the daemon only over an owner-private Unix socket
  (.engineering/planning/epic/rust-sdk.md:26).
- The run cannot confine a child itself. The sandbox passes `--unshare-user --disable-userns`
  (crates/substrate-host/src/process.rs:74), so no bubblewrap can nest inside a box.
- The PTY mechanism exists only on the daemon's session path: `pty::open` and `resize`
  (crates/substrate-host/src/pty.rs:92, 225), and the controlling terminal taken inside the
  sandbox (crates/substrate-host/src/process.rs:2830-2836). The SDK starts such a session from
  outside a box (`pty_session`, crates/b10x-substrate-sdk/src/lib.rs:797; `resize`, :1600).

### How this relates to the draft ADR on `adr/workspace-unix-sockets`

The draft (`adr/0048-confined-unix-sockets-are-scoped-to-the-workspace-by-landlock.md` on that
branch, status proposed) does not close this gap, and this story does not duplicate it:

- It admits `AF_UNIX` only for a start that opts in with `unix_sockets: "workspace"` (draft lines
  67-75). Bind and connect stay confined beneath `/workspace`, and a connect to a host socket stays
  `EACCES` (draft lines 91-92). The daemon's socket therefore stays unreachable.
- It needs Landlock ABI 9, which means Linux 7.1 (draft line 84). Below that, every opted-in start
  is refused (draft lines 120-125).
- Its residual says a host service that binds a socket inside a workspace directory becomes
  reachable, and that placing it there is the operator's act (draft lines 146-148). A per-workspace
  daemon endpoint beneath `/workspace` would build on exactly that residual. It would inherit the
  7.1 floor and make the residual a designed path.
- Its number collides with the accepted
  `adr/0048-the-wire-states-what-a-confinement-adapter-proves-never-its-name.md` on main. This
  story's ADR takes the next free number.

## Acceptance

- **An ADR first** (invariant 8), amending ADR 0015. It decides:
  - the channel. Options: a pre-connected descriptor the daemon hands to the run at start, which
    needs no `socket(2)` and so no kernel floor; or a daemon-bound endpoint beneath `/workspace`
    under the draft ADR;
  - the scope. The endpoint serves session start, attach, input, resize, signal and observe for
    the run's own workspace only. Any other workspace, the host-process routes and workspace
    lifecycle are refused by name;
  - who owns a nested session: the workspace's owner, so the host-side SDK client attaches to it;
  - the opt-in start member, the capability fact, the applied record member and the named
    refusals;
  - why the in-process `local` adapter with a terminal (`story:confinement-runs-on-a-terminal`) is
    or is not enough. That adapter keeps the session out of the daemon's ledger and makes it
    attachable only through the run.
- A successor contract bundle publishes what the ADR decides (invariant 6). Each member is held to
  a `substrate_wire` constant.
- A delegated-lane case: an exec started with the opt-in starts a PTY session in its own workspace
  through the channel. The host-side SDK attaches to that session, writes a line, reads it echoed
  and resizes it. A start naming another workspace through the channel is refused and recorded as
  refused.
- Portable lane: where the fact is absent, a start naming the opt-in is refused by name before
  dispatch.

## Boundaries

- No general daemon access from a box: no host-process routes, no workspace create or delete, no
  sessions in another workspace.
- No nested user namespaces: `--disable-userns` stays.
- Writable roots in the SDK are `story:sdk-writable-roots`. Unix sockets for tmux and similar tools
  are the draft ADR's story.
