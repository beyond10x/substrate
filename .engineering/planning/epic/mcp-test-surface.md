---
format: aep.planning-md/1
id: epic:mcp-test-surface
kind: epic
status: draft
title: MCP test surface for arbitrary harnesses
summary: A disposable daemon and protocol-neutral MCP adapter make Substrate easy to exercise from Codex and other MCP-capable harnesses.
owner: substrate
tags:
- deferred
- mcp
- testing
revision: 1
---
# Epic: MCP test surface

## Outcome

Arbitrary MCP-capable harnesses can start an isolated, disposable Substrate daemon and exercise its public workspace, execution, output, metrics and cleanup behavior through a small MCP adapter.

## Boundaries

The adapter is a client of the public Substrate wire contract. No MCP type or server implementation enters the daemon, host driver or wire crate; the surface is for local testing and evaluation, not a production multi-tenant ingress. The work starts after the current resource-bounded execution epic.
