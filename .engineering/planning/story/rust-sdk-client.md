---
format: aep.planning-md/3
id: story:rust-sdk-client
kind: story
status: implemented
title: The Rust SDK exposes typed workspace and execution handles
summary: Builders, typed handles and ledger-aware recovery cover the daemon-advertised core contract.
owner: substrate
tags:
- sdk
relations:
- decomposes: epic:rust-sdk
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-08-31T22:49:55Z", actor: "human:timo", revision: 2, imported: true}
- {from: "proposed", to: "active", at: "2026-08-31T22:49:55Z", actor: "human:timo", revision: 3, imported: true}
- {from: "active", to: "implemented", at: "2026-08-31T22:49:59Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}, imported: true}
---
# Story: The Rust SDK exposes typed workspace and execution handles

## Outcome

A Rust caller connects over a Unix socket, verifies the advertised contract, creates an empty workspace, performs bounded file operations, runs or starts an argv-only process, observes output and events, and reconciles an unanswered mutation without assembling HTTP or JSON manually.

## Acceptance

The SDK owns its public models, requires an explicit execution policy, preserves the daemon refusal taxonomy, reuses one caller-minted operation id across ambiguous transport recovery, and has no public escape hatch to the host driver or daemon application.
