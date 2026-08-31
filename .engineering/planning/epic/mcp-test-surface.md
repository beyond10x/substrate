---
format: aep.planning-md/1
id: epic:mcp-test-surface
kind: epic
status: proposed
title: MCP test surface for arbitrary harnesses
summary: A disposable daemon and protocol-neutral MCP adapter make Substrate easy to exercise from Codex and other MCP-capable harnesses.
owner: substrate
tags:
- deferred
- mcp
- testing
relations:
- depends_on: epic:resource-bounded-execution
revision: 3
---
# Epic: MCP test surface

## Outcome

Arbitrary MCP-capable harnesses can start an isolated, disposable Substrate daemon and exercise its public workspace, execution, output, metrics and cleanup behavior through a small MCP adapter.

## Boundaries

The adapter is a client of the public Substrate wire contract. No MCP type or server implementation enters the daemon, host driver or wire crate; the surface is for local testing and evaluation, not a production multi-tenant ingress. The work starts after the current resource-bounded execution epic.

## Reconciled state — 2026-09-01

Resource-bounded execution is implemented, so the former deferral is cleared. The MCP surface remains a proposed local testing adapter after SDK contract parity; it is not a production ingress and does not block remote HTTPS serving or driver work.
