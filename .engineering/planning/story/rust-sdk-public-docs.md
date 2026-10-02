---
format: aep.planning-md/3
id: story:rust-sdk-public-docs
kind: story
status: implemented
title: The public site teaches the Rust SDK journey
summary: Reader-facing examples cover connection, managed mode, explicit limits and current development boundaries.
owner: substrate
tags:
- docs
- sdk
relations:
- decomposes: epic:rust-sdk
- depends_on: story:rust-sdk-client
- depends_on: story:rust-sdk-managed-daemon
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-08-31T22:49:55Z", actor: "human:timo", revision: 2, imported: true}
- {from: "proposed", to: "active", at: "2026-08-31T22:49:55Z", actor: "human:timo", revision: 3, imported: true}
- {from: "active", to: "implemented", at: "2026-08-31T22:50:00Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}, imported: true}
---
# Story: The public site teaches the Rust SDK journey

## Outcome

A reader arriving cold can connect to a daemon or start an owned child, create a workspace and run one explicitly bounded command from Rust.

## Acceptance

The website is self-contained, marks the SDK and contract as development status, links no internal designs or work records, and states that managed means a separate daemon process rather than in-process execution.
