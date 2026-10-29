---
format: aep.planning-md/3
id: story:confined-unix-sockets-scoped-to-the-workspace
kind: story
status: draft
title: Confined Unix sockets are scoped to the workspace by Landlock
summary: Opt-in AF_UNIX for tmux, screen and language servers; bind and connect confined beneath /workspace by Landlock RESOLVE_UNIX (Linux 7.1+), refused by name elsewhere (ADR 0048).
relations:
- informed_by: story:a-confined-process-cannot-mutate-host-global-kernel-state
revision: 1
---
# Story: confined Unix sockets are scoped to the workspace

## Why

The operator decided on 2026-10-29 that a confined process may create `AF_UNIX` sockets, provided
`bind` and `connect` stay inside the workspace, so that tmux, screen and language servers work in a
sandbox. A host service whose socket is visible through a read-only mount must stay unreachable.

Today the seccomp profile refuses `socket(AF_UNIX)` with `EACCES` (ADR 0015, ADR 0039 decision 2).
The reason still holds: a read-only bind mount and a network namespace do not stop `connect(2)` to a
pathname socket. This was measured on a 6.18 kernel, ADR 0048 E1 and E2.

ADR 0048 (`adr/0048-confined-unix-sockets-are-scoped-to-the-workspace-by-landlock.md`, proposed)
decides the mechanism:

- a start opts in with `unix_sockets: "workspace"`;
- the seccomp profile then admits stream and seqpacket `AF_UNIX` sockets;
- a Landlock domain applied inside the sandbox handles `MAKE_SOCK` and `RESOLVE_UNIX` beneath
  `/workspace` and scopes abstract sockets;
- a second probe proves the scope and publishes `exec.unix-sockets`;
- a missing fact is the refusal `exec.unix-sockets-unserved` (501).

`RESOLVE_UNIX` is Landlock ABI 9, Linux 7.1. Below that kernel, the capability is absent and refused
by name.

## Order

1. ADR 0048 is accepted by the operator. Its status is `proposed`, which `cargo xtask check-adrs`
   refuses, so it cannot merge before acceptance.
2. A successor bundle after `0.25.0` publishes four things, each held to a `substrate_wire`
   constant: the `unix_sockets` start member on the exec and session starts, the
   `exec.unix-sockets` fact, the applied `unix_sockets` member and the
   `exec.unix-sockets-unserved` refusal.
3. Code comes only after both (invariant 8).

## What changes

In `crates/substrate-host`:

- `src/seccomp.rs`: `profile` and `filters` take the opt-in and emit a second program. In it,
  `socket(AF_UNIX)` is admitted only for type `SOCK_STREAM` or `SOCK_SEQPACKET` (masked `0xf`)
  and protocol `0`. A new `Label` carries the type check. `ALLOWED_SOCKETS` stays the default
  program's list. The `FAMILY_POLICY` row for `AF_UNIX` records both programs. The `seccomp::tests`
  cases assert the default program is unchanged and the opted-in one admits exactly those triples.
- A new module, for example `src/unix_scope.rs`, holds the in-sandbox launcher. The launcher:
  - opens `/workspace` `O_PATH`;
  - creates a ruleset that handles `LANDLOCK_ACCESS_FS_MAKE_SOCK | LANDLOCK_ACCESS_FS_RESOLVE_UNIX`
    with `scoped = LANDLOCK_SCOPE_ABSTRACT_UNIX_SOCKET`;
  - adds one path-beneath rule on `/workspace` and restricts itself;
  - execs the workload, or exits before exec on any Landlock error.

  The ABI query (`LANDLOCK_CREATE_RULESET_VERSION` at least 9) also lives in this module.
- `src/process.rs`, `ProcessRuntime::command`: for an opted-in start, binds the launcher read-only
  by descriptor at a reserved mount and puts it before the `/usr/bin/env -u PWD` prefix (after
  `CONTROLLING_TERMINAL_ARGV` in a pty session). Uses the opted-in seccomp program. Records
  `unix_sockets` in the applied record.
- `src/read_only_roots.rs`, `admit`, and the other mount-admission sites: the launcher's mount
  joins the Substrate-owned mounts a declared root may not equal or sit beneath.
- `src/probe.rs`, `probe`: after the unchanged `probe_bubblewrap` floor, and only when `exec`
  holds and the ABI is at least 9, a new probe runs one throwaway sandbox. It reuses the sentinel
  from `crate::private_socket::bind_private`. Only when every observation in ADR 0048 decision 4
  holds does it set the `exec.unix-sockets` fact. Pin its argv beside `bubblewrap_probe_command`
  so that a case needing no socat or backend on disk checks the argv.
- The admission check for the start member refuses `exec.unix-sockets-unserved` before dispatch
  when the fact is absent. Every other adapter keeps the fact absent and the refusal.

## Acceptance

Delegated-lane cases each run their scenario once and finish in seconds. They run through
`bash scripts/delegated-lane.sh`, in `crates/substrate-host/tests/` and in `DELEGATED_CASES` of
`crates/substrate-daemon/tests/runtime_vectors.rs` with the count raised.

On a delegated host with Linux 7.1 or later (Landlock ABI 9 or later):

1. The snapshot carries `exec.unix-sockets` = `{"scope": "workspace"}` beside `exec.no-egress`.
2. In an opted-in exec, one process binds and listens on a stream socket beneath `/workspace`, and
   a second process in the same exec connects to it and exchanges one line.
3. A second opted-in exec in the same workspace connects to the socket the first, still running,
   bound. The applied record of both says `unix_sockets: "workspace"`.
4. `connect` to a host socket inside a declared read-only root is `EACCES`, and the host listener
   accepts nothing.
5. `connect` through a symlink beneath `/workspace` that points at that host socket is `EACCES`.
6. `bind` beneath `/tmp` is `EACCES`.
7. `connect` to an abstract name a host process listens on is refused.
8. `socket(AF_UNIX, SOCK_DGRAM, 0)` is `EACCES`.
9. A start that does not opt in still gets `EACCES` from `socket(AF_UNIX, SOCK_STREAM, 0)`, and the
   floor probe's sentinel assertion in `probe_bubblewrap` is unchanged.

On every host, including Linux below 7.1 and the portable lane:

10. Where the fact is absent, a start naming `unix_sockets` is refused `exec.unix-sockets-unserved`
    (501) before dispatch and recorded refused. Nothing runs.

Cases 1 to 9 are absent, never reported as passed, on a kernel below 7.1. The lane names them as
absent. The 6.18 workstation that wrote ADR 0048 cannot run them (ADR 0048 E7).
