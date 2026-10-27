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

## Running Git in a linked worktree

A linked worktree's `.git` is a file naming `gitdir: <common dir>/worktrees/<name>`, a host path
outside the workspace, so inside an exec Git finds no repository. Declare the common directory —
`git rev-parse --path-format=absolute --git-common-dir` on the host — as one more entry in
`read_only_roots`, mounted at its own host path:

```json
"read_only_roots": [
  {"host_path": "/srv/repos/app/.git", "mount": "/srv/repos/app/.git"}
]
```

Commands that only read, such as `git status` and `git log`, then work. A declared root is always
mounted read-only, so commands that write the index, refs or objects (`git add`, `git commit`,
`git fetch`) are refused with a read-only file system error. Several directories are several entries,
up to the bound the contract publishes (four). A mount point beneath a path Substrate owns, such as
`/tmp`, is refused, so a repository under `/tmp` cannot be declared at its own path.

## Admitting one host file

A program or a configuration file that lives in a directory beside many others can be admitted
alone. Declare it in `read_only_files`, and put its mount's directory on the `PATH` you set:

```json
"env": {"allow": [], "set": {"PATH": "/opt/gates/bin:/usr/bin"}},
"read_only_files": [
  {"host_path": "/srv/tools/bin/gate", "mount": "/opt/gates/bin/gate"}
]
```

The file is bound read-only with its host mode bits, so `gate` runs by name, a write to it fails,
and nothing else from `/srv/tools/bin` is visible. The host path must be an existing regular file
reached without any symlink. Both paths are absolute and canonical, contain no NUL, and are at
most 4000 bytes with no component over 255. The mount must lie outside every path Substrate owns
and every declared root, and be unique. A start may declare up to the bound the machine's
`exec.read-only-files` fact publishes (16). Each rejection is a named refusal before dispatch, and
the applied observation lists every bound file. The program holds no descriptor on the host file:
the only way to it is the read-only mount.

## One contract, several mechanisms

The guarantees above are the Linux host driver's: the daemon's floor and the `linux` confinement
adapter's. The host crate offers five more adapters behind the same contract, in the embedding
process: `local`, `macos`, `systemd`, `oci` and `pod`. Each proves its own families and facts when
it opens. A profile that needs one an adapter did not prove is refused by name, and nothing falls
back to another adapter. `local` is declared insecure. Read
[choose a confinement profile](../guides/confinement-profiles.md) for what each adapter serves and
how to pick one by its facts.

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
