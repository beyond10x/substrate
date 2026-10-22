---
format: aep.planning-md/3
id: story:exec-working-directory
kind: story
status: implemented
title: A confined start names its working directory
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-22T04:37:18Z", actor: "agent:b10x-chef-orchestrator", revision: 3, executor: "agent:substrate-wave-20261022", correlation: "wave-2026-10-22-confinement-gaps"}
- {from: "proposed", to: "active", at: "2026-10-22T04:37:18Z", actor: "agent:b10x-chef-orchestrator", revision: 4, executor: "agent:substrate-wave-20261022", correlation: "wave-2026-10-22-confinement-gaps"}
- {from: "active", to: "implemented", at: "2026-10-22T08:41:43Z", actor: "agent:b10x-chef-orchestrator", revision: 5, decided_on: {"recorded":{"test_result":1}}}
---
# A confined start names its working directory

Loom 0.33.0 starts every child in `/workspace` because Substrate offers no other directory
(`loom/crates/loom-intake-slice/src/confinement.rs`, C:974-976). Design
`docs/design/26-working-directory-and-several-apertures.md` § 1 adds an optional `workdir` to
`ExecStartInput` (and so to session start), bundle `0.21.0`.

## Acceptance

- An exec started with `workdir` `/workspace/sub` (existing) prints `/workspace/sub` from `pwd`.
- A `workdir` at a declared read-only root's mount, and at `/scratch` with scratch set, is admitted.
- `workdir` `/tmp`, `/usr` or `/` is refused `exec.workdir-outside-mounts` (422) before dispatch.
- `workdir` with `..`, a trailing `/`, a relative path or over 4096 bytes is `exec.workdir-invalid` (422).
- A `workdir` that does not exist, or is a symlink, is `exec.workdir-missing` (422); nothing runs.
- A start without `workdir` serializes, hashes and runs exactly as on `0.20.0`.
- Portable lane: `exec.workdir-unserved` (501); delegated lane: the cases above through the shipped daemon.

## Scope

`crates/substrate-wire/src/lib.rs`, `crates/substrate-host/src/process.rs` (the `--chdir` argument),
`crates/substrate-host/src/pty.rs` if it builds its own argv, the daemon admission path,
`xtask/bundle-source/0.21.0/`, `contracts/substrate-wire/0.21.0/`,
`crates/substrate-daemon/tests/runtime_vectors.rs`.
