---
format: aep.planning-md/3
id: story:a-client-cli-drives-the-daemon
kind: story
status: implemented
title: A client CLI drives the daemon over its socket
revision: 5
transitions:
- {from: "draft", to: "proposed", at: "2026-10-16T08:16:47Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-16T08:16:47Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-16T16:50:42Z", actor: "human:timo", revision: 5, decided_on: {"recorded":{"test_result":1}}}
---
# A client CLI drives the daemon over its socket

## Why

Only `substrate-daemon` is a binary a person or an agent session can run today. Every use of the
daemon goes through the Rust SDK from code, or through hand-written HTTP over the Unix socket. A
session that wants to see what the daemon holds, or stop what it started, has no command to type.

## What

A new workspace package `b10x-substrate-cli` builds one binary, `b10x-substrate`, with a clap
derive command line. It is a client of the existing socket API through `b10x-substrate-sdk` only;
it adds no route, no wire field and no daemon behaviour, so it needs no successor bundle and no ADR.

Commands cover the served surface the SDK exposes: the machine and its capability facts, workspaces
(create, show, renew, destroy), exec (run a command in a workspace, show an exec), sessions (show,
signal), operations (show), metrics and events. `--socket` selects the daemon socket, falling back to
`SUBSTRATE_SOCKET` (the daemon has no built-in default). The global `--output human|json` selects
the rendering, as the ESS binding states; human output is a table or a line.
A daemon refusal is printed with its named code and the CLI exits non-zero; a capability the
daemon does not advertise is reported absent, never assumed.

## Specification

The command line is an ESS CLI presentation binding (`spec/client/cli.yaml`) over the commands and
views it exposes, validated with `ess specify cli`. The clap derive command line is held to the
generated `spec/generated/cli/binding.json` by `cli-binding-validates`. What the binding cannot
state is kept in the crate and named in `spec/coverage.md`: `exec run` passes the command's bytes
through and exits with its code, and a daemon refusal code travels in `error.data.code`.

## Acceptance

Named scenarios, run by `cargo test -p b10x-substrate-cli` against the shipped daemon binary over
a Unix socket:

- `cli-machine-shows-facts`: `b10x-substrate --output json machine` prints the daemon's capability facts.
- `cli-workspace-lifecycle`: create, show, renew and destroy an empty workspace; each step's JSON
  matches the daemon's observation.
- `cli-exec-runs-and-reports-exit`: `exec run` prints the command's output and exits with its code,
  where the confined exec capability is served; where it is not, the named refusal is printed and the
  exit is non-zero.
- `cli-refusal-is-named`: destroying an unknown workspace prints the daemon's refusal code.
- `cli-binding-validates`: `ess specify cli` accepts the binding, and every clap leaf is named in it.

## Scope

`crates/b10x-substrate-cli/` (new), `Cargo.toml` members, `Cargo.lock`, `spec/` (binding),
`website/docs/` (one guide page), `README.md`, `CHANGELOG.md`.
