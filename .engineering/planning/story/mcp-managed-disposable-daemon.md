---
format: aep.planning-md/1
id: story:mcp-managed-disposable-daemon
kind: story
status: proposed
title: Launch a disposable Substrate daemon behind MCP
summary: A local MCP adapter owns one fresh daemon lifecycle and projects its public operations for harness testing.
owner: substrate
tags:
- deferred
- mcp
- testing
relations:
- decomposes: epic:mcp-test-surface
- depends_on: story:sdk-promoted-contract-parity
revision: 2
---
# Story: Launch a disposable daemon behind MCP

## Outcome

One command starts a fresh private Substrate daemon, waits for its machine facts, exposes bounded workspace and execution operations as MCP tools, and tears down the socket, database, workspaces and child process when the MCP server exits.

## Acceptance

The adapter verifies the advertised contract and capability snapshot, preserves operation IDs and named refusals, never bypasses the daemon through host-driver APIs, exposes output and exact resource observations without inventing fields, scopes all state to the disposable instance, and includes a Codex-compatible hands-on smoke test. Unsupported host capabilities remain absent or refused. No TCP listener or production authentication claim is introduced.
