---
format: aep.planning-md/3
id: story:read-only-host-file-bind
kind: story
status: active
title: A single host file is admitted read-only
relations:
- decomposes: epic:host-run-gates
scope:
- confidence: cited
  path: adr/0033-a-single-host-file-is-admitted-read-only.md
- confidence: cited
  path: contracts/substrate-wire/0.18.0
- confidence: cited
  path: crates/substrate-host/src/process.rs
- confidence: cited
  path: crates/substrate-wire
- confidence: cited
  path: xtask/bundle-source/0.18.0
revision: 9
transitions:
- {from: "draft", to: "proposed", at: "2026-10-11T19:53:37Z", actor: "human:timo", revision: 3}
- {from: "proposed", to: "active", at: "2026-10-11T19:54:15Z", actor: "human:timo", revision: 9}
---
# Story: a single host file is admitted read-only

## Problem

ADR 0010 admits declared host **directories** read-only; a root whose host path is not an existing
directory is refused. A caller that needs one program (a binary in a directory that holds many
others) or one configuration file (a user-level tool configuration) has to bind the whole
directory around it, which admits everything else in it, or copy the file into the workspace.

## Contract

A new, optional, bounded start field beside `read_only_roots`, so existing roots keep their exact
meaning and validation. It rides successor bundle `0.18.0` with
`story:exec-usage-without-io-controller`; ADR 0033 records it before code.

- `read_only_files: [{ host_path, mount }]` on exec and raw-pipe start. `host_path` is absolute,
  canonical and an existing regular file reached without a symlink; `mount` is absolute, canonical,
  outside every substrate-owned mount and every declared root, and unique.
- The capability snapshot publishes the served bound (`MAX_READ_ONLY_FILES`); the applied
  confinement observation lists every file bound, as it does roots.
- The mount is `--ro-bind` of the file; executable bits are the host file's.

## Specification

Decision Q3 of `decision-blocker:host-run-gates-contract-route`: filesystem confinement stays
outside the ESS projection (`spec/coverage.md`); the story is proved by host tests, the runtime
vectors and the delegated lane.

## Acceptance

- An exec declaring a host program as a read-only file at a mount on its `PATH` runs it.
- The same exec cannot write the file, and sees no sibling of it from the host directory.
- A directory, a symlinked path, a mount colliding with an owned mount or a declared root, a
  duplicate mount, and a list over the bound are each refused before dispatch with a named code.
