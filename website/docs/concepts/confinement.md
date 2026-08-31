---
title: Confinement and refusal
description: The guarantees Substrate verifies before it admits filesystem and process effects.
---

# A missing guarantee is a refusal

Confinement is a set of independently checked guarantees, not a single “sandboxed” label. Substrate
advertises execution only after the active host has proved the minimum set it needs.

## Guarded filesystem access

Workspace file operations resolve beneath a pre-opened root. The guard rejects:

- absolute paths and lexical traversal;
- symbolic-link, magic-link, and mount escape;
- ambiguous dangling links;
- writes that cannot be committed as bounded atomic replacements.

A refused path leaves the outside filesystem untouched.

## Process isolation

An admitted host exec receives:

- an argument vector rather than a shell string;
- a cleared environment followed by an explicit non-secret baseline and admitted values;
- a separate writable `/workspace`;
- read-only system inputs and, when requested, a digest-verified read-only `/runtime` capsule;
- no usable network interface in the minimum no-egress profile;
- a syscall filter that refuses new Unix-domain sockets and `io_uring_setup`, closing host-IPC and
  asynchronous-I/O paths that namespace isolation alone does not remove;
- PID and memory-plus-swap bounds with cumulatively observed CPU;
- bounded stdout and stderr capture that keeps draining after truncation;
- timeout, cancellation, and whole-cgroup termination.

Requested isolation and applied isolation are recorded separately. The final observation says what
the machine actually applied.

## Capability snapshots

Admission binds an operation to a probed backend and configuration generation. Security-critical
facts are checked again before dispatch. If the backend disappears or its identity changes, the
operation is refused against the stale snapshot instead of racing ahead.

## Refusal is part of the contract

Common examples include:

| Outcome | Meaning |
|---|---|
| `exec.sandbox-unavailable` | the host cannot enforce the minimum execution profile |
| `workspace.path-escape` | a requested path would leave the guarded tree |
| `workspace.source-unserved` | this deployment does not serve the requested source kind |
| `unserved` | the selected deployment does not implement the operation |
| `exhausted` | a declared capacity or resource bound has been reached |

These are answered outcomes. They tell the caller whether to change the request, select another
deployment, or wait for capacity. They never masquerade as successful execution.

## What the minimum host does not claim

The boundary does not claim protection from a compromised kernel or comprehensive syscall
allowlisting. Its seccomp rules close the named AF_UNIX and io_uring paths; they are part of the
minimum confinement floor, not a claim that every kernel interface has been reduced to an allowlist.
A development execution capsule verifies the capsule's own bytes; it does not attest the host
kernel, interpreter, libraries, or base system.

Read [operations and observations](./operations.md) to see how refusals and uncertain outcomes are
made durable.
