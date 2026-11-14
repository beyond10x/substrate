---
format: aep.planning-md/3
id: story:test-fifos-made-in-process
kind: story
status: active
title: Tests make their FIFOs in process, so a runner's mkfifo cannot fail the gate
summary: Five test helpers create FIFOs with nix::unistd::mkfifo instead of running the external mkfifo command, which on ubuntu-latest now refuses a non-UTF-8 path.
owner: substrate
tags:
- tests
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-14T20:14:06Z", actor: "human:agent", revision: 3, executor: "human:timo"}
- {from: "proposed", to: "active", at: "2026-11-14T20:14:06Z", actor: "human:agent", revision: 4, executor: "human:timo"}
---
# Story: Tests make their FIFOs in process

## Outcome

A pull request's *Gate: tests and Clippy* no longer fails because of which `mkfifo` the runner
ships: every test that needs a FIFO makes it with `nix::unistd::mkfifo`, not the external command.

## Context

On 2026-11-14 *Gate: tests and Clippy* failed on #219 (run 43614631820) and #220 (run 43615670301)
at the same case, `v1_every_page_by_cursor_carries_the_directory_count_adversary_pass1` in
`crates/substrate-daemon/tests/omitted_entries_adversary_pass1.rs`. Its helper ran `mkfifo` on a
path holding a non-UTF-8 name, and the `ubuntu-latest` runner's `mkfifo` now refuses it:
`error: invalid UTF-8 was detected in one or more arguments`. The daemon under test was never
reached; the fixture could not be built.

Five places run the command: that helper, `crates/substrate-daemon/tests/http.rs`,
`crates/substrate-daemon/tests/runtime_vectors.rs`, `crates/b10x-substrate-sdk/tests/managed.rs`
and `crates/substrate-kubernetes-client/src/client_tests.rs`. Only the first passes a non-UTF-8
path today; all five move, so a FIFO fixture depends on no external tool at all.

## Acceptance

- No `Command::new("mkfifo")` remains in the workspace.
- The five affected cases pass locally, the omitted-entries suite included.
- `cargo clippy --workspace --all-targets -D warnings` is clean.
- *Gate: tests and Clippy* passes on the pull request.

## Out of Scope

- Any change to the daemon, the SDK or the Kubernetes client outside their tests.

## Open Questions

None.
