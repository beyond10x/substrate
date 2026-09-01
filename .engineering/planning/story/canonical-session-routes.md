---
format: aep.planning-md/1
id: story:canonical-session-routes
kind: story
status: implemented
title: Session is the canonical route and SDK resource
summary: The breaking 0.15.0 contract serves the existing session operations only at /v1/sessions.
owner: substrate
tags:
- session
- wave/remote-foundation-01
- wire
relations:
- decomposes: epic:remote-serving
- depends_on: story:contract-gate-sees-route-paths
revision: 8
---
# Story: Rename the session route family

## Outcome

The existing eight session operations move from `/v1/pipe-sessions` to `/v1/sessions`. The old paths are not registered and there is no compatibility alias.

## Design closure

The operator accepted the breaking route migration in `docs/design/16-sessions-are-not-pipe-sessions.md`, Substrate ADR 0028 and Atlas ADR 0022. The successor is closed to the exact eight path replacements, preserves all non-session routes and operation semantics, and leaves every earlier bundle byte-identical.

## Acceptance

1. Successor bundle `0.15.0` names predecessor `0.14.0`, records exactly eight removed and eight added route addresses, and the checker refuses any other removal, addition or semantic drift.
2. The daemon registers only `/v1/sessions`; every legacy `/v1/pipe-sessions` request returns the ordinary route-not-found response on Unix, development TCP and hosted TLS.
3. The SDK sends only the renamed routes; tests, contract-derived runtime inventory, MCP references, examples and public documentation use them.
4. Operation ids, request/result schemas, Rust resource types, modes and byte-plane vocabulary do not change as part of this route-only migration.
5. The full gate, genuinely delegated lane and planning validation pass.

## Out of Scope

Renaming Rust types, schema authority files, operation ids, or the `pipes` I/O mode.
