---
format: aep.planning-md/3
id: story:read-only-host-file-bind
kind: story
status: draft
title: A single host file is admitted read-only
relations:
- decomposes: epic:host-run-gates
revision: 1
---
# Story: a single host file is admitted read-only

## Problem

ADR 0010 admits declared host **directories** read-only; a root whose host path is not an existing
directory is refused. A caller that needs one program (a binary in a directory that holds many
others) or one configuration file (a user-level tool configuration) has to bind the whole
directory around it, which admits everything else in it, or copy the file into the workspace.

## Contract

A new, optional, bounded start field beside `read_only_roots`, so existing roots keep their exact
meaning and validation:

- `read_only_files: [{ host_path, mount }]` on exec and raw-pipe start. `host_path` is absolute,
  canonical and an existing regular file reached without a symlink; `mount` is absolute, canonical,
  outside every substrate-owned mount and every declared root, and unique.
- The capability snapshot publishes the served bound (`MAX_READ_ONLY_FILES`); the applied
  confinement observation lists every file bound, as it does roots.
- The mount is `--ro-bind` of the file; executable bits are the host file's.

This needs a successor wire bundle and an ADR before code (invariants 6 and 8). It can ride the same
successor bundle as `story:exec-usage-without-io-controller` if both are approved. Not built until
then.

## Acceptance

- An exec declaring a host program as a read-only file at a mount on its `PATH` runs it.
- The same exec cannot write the file, and sees no sibling of it from the host directory.
- A directory, a symlinked path, a mount colliding with an owned mount or a declared root, a
  duplicate mount, and a list over the bound are each refused before dispatch with a named code.
