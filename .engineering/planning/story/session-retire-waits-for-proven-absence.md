---
format: aep.planning-md/3
id: story:session-retire-waits-for-proven-absence
kind: story
status: implemented
title: A session in unknown is retired only after its tree is recorded gone
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-10-23T08:41:48Z", actor: "agent:b10x-chef-orchestrator", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-23T08:41:48Z", actor: "agent:b10x-chef-orchestrator", revision: 3}
- {from: "active", to: "implemented", at: "2026-10-23T14:58:56Z", actor: "agent:b10x-chef-orchestrator", revision: 4, decided_on: {"recorded":{"test_result":1,"review_outcome":1,"verification":2}}}
---
# Story: A session in `unknown` is retired only after its tree is recorded gone

## Context

Found by the 2026-10-23 wave (unit u2, adversary pass 2 and coordinator review). `retire_exec`
now admits an `unknown` exec only once recovery has recorded it physically absent
(`crates/substrate-store/src/execs.rs`, `exec_recorded_absent`). `session.retire`
(`crates/substrate-store/src/sessions.rs:337-344`) still admits a session in `Unknown` without that
check, so a session whose process tree was never proven empty can be retired and its row reclaimed.
Pre-existing; left out of u2 because closing it adds a refusal to a route.

## Acceptance

A session in `unknown` whose tree is not recorded physically absent is refused retire with a named
code before its row changes, and one recorded absent is retired; a store case proves both, and the
refusal is published in a successor bundle or the existing `exec.not-terminal` code is reused and
the story says which.
