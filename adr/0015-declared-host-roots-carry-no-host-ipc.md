---
status: accepted
date: 2026-08-31
---

# ADR 0015: declared host roots carry no host IPC

## Context

[ADR 0010](0010-declared-host-roots-are-mounted-read-only.md) permits operator-declared host
directories in a confined run. Read-only filesystem access does not make a Unix-domain socket
read-only: a process that can resolve the socket path can connect to the host service. A caller can
also currently place a root below `/runtime` and shadow the verified execution capsule.

## Decision

Every confined process runs under a verified seccomp program that denies creation of
`AF_UNIX`/`AF_LOCAL` sockets and the compatible `socketcall` path, while preserving `socketpair`
for process-local IPC. `io_uring_setup` is denied so socket creation cannot bypass the syscall
filter. The program validates its audit architecture and an unsupported architecture is a named
sandbox refusal.

The startup confinement probe mounts a private directory containing a sentinel host Unix socket
and proves that a confined child cannot connect to it. `exec.no-egress` is published only after
that probe and the existing network-namespace proof both succeed.

Read-only-root mount points are compared component by component. A root equal to or below a
Substrate-owned mount, or overlapping another declared root, is refused as
`exec.read-only-root-invalid`; nothing is silently relocated. `/` is reserved only by exact match,
so a name such as `/runtime2` is not confused with `/runtime`.

## Consequences

Harness toolchains and data directories remain usable as ordinary read-only files, but a mounted
socket cannot become undeclared host authority and no caller root can replace a verified capsule or
other owned filesystem. A host that cannot install and prove the filter publishes no exec fact.
