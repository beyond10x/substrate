---
format: aep.planning-md/1
id: story:mcp-managed-disposable-daemon
kind: story
status: implemented
title: Launch a disposable Substrate daemon behind MCP
summary: A local MCP adapter owns one fresh daemon lifecycle and projects its public operations for harness testing.
owner: substrate
tags:
- deferred
- mcp
- testing
- wave/remote-foundation-01
relations:
- decomposes: epic:mcp-test-surface
- depends_on: story:sdk-promoted-contract-parity
revision: 7
---
# Story: Launch a disposable daemon behind MCP

## Outcome

One command starts a fresh private Substrate daemon, waits for its machine facts, exposes bounded workspace and execution operations as MCP tools, and tears down the socket, database, workspaces and child process when the MCP server exits.

## Acceptance

The adapter verifies the advertised contract and capability snapshot, preserves operation IDs and named refusals, never bypasses the daemon through host-driver APIs, exposes output and exact resource observations without inventing fields, scopes all state to the disposable instance, and includes a Codex-compatible hands-on smoke test. Unsupported host capabilities remain absent or refused. No TCP listener or production authentication claim is introduced.

## Design-closure audit — 2026-09-01

Implementation requires `docs/design/19-mcp-disposable-test-adapter.md` to be accepted after SDK parity. The adapter remains a private, development-only stdio composition over the SDK and Unix socket; it is not production ingress. ADR 0025 separately accepts public OCI distribution of that test-only binary without making the crate publishable or the MCP surface stable.

The design must fix: a private SDK-only crate boundary; a custom bounded JSONL transport rather than an unbounded stdio helper; caller-supplied operation ids on every mutation; exact refusal/observation projection; no model-selected host roots, secret slots or apertures; tracked exec/workspace teardown before daemon shutdown; an exclusive process-free delegated cgroup root; and honest SIGKILL semantics. Portable OCI smoke proves the named sandbox refusal; positive execution stays in the native delegated lane until a separate container-runtime design supplies the confinement prerequisites.

Acceptance additionally depends on the managed-daemon child-absence fix and public SDK output/metrics/absence-preserving facts. `cargo xtask check-mcp-boundary` must prove the crate is private, depends on Substrate only through the SDK, enables no HTTP/OAuth MCP features and uses no unbounded rmcp stdio transport.
