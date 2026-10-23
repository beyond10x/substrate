---
format: aep.planning-md/3
id: story:host-admit-checks-the-read-only-roots-fact
kind: story
status: implemented
title: The host driver admits read-only roots only when it advertises the fact
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-23T08:41:50Z", actor: "agent:b10x-chef-orchestrator", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-23T08:41:51Z", actor: "agent:b10x-chef-orchestrator", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-23T14:58:55Z", actor: "agent:b10x-chef-orchestrator", revision: 4, decided_on: {"recorded":{"test_result":1,"review_outcome":1,"verification":2}}}
---
# Story: The host driver admits read-only roots only when it advertises the fact

## Context

Found by the 2026-10-23 wave (unit u3). The daemon now refuses a start naming `read_only_roots` with
501 `exec.sandbox-unavailable` when the snapshot lacks `exec.read-only-roots`
(`crates/substrate-daemon/src/app/operations.rs`, `check_read_only_roots`). The host driver's own
`admit` does not check the fact, unlike `read_only_files::admit`; closing it needed edits at
`crates/substrate-host/src/process.rs:2099` and `:6899` and a signature change, outside u3's lines.
Today the daemon check and the probe agree by construction; a second embedder of the host crate
gets no refusal.

## Acceptance

The host driver refuses a start naming read-only roots when its own capability snapshot lacks
`exec.read-only-roots`, with the same code the daemon uses, proven by a host unit case that calls
the driver directly.
