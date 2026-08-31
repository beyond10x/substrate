---
status: accepted
date: 2026-08-31
---

# ADR 0018: one registry declares every served API major

## Context

The daemon serves five `/v2` workspace routes that no released bundle declares. Its response
builder also emits `api_version: "v1"` for those routes. A registry that omits served routes cannot
be the contract clients pin, and splitting registries would make cross-major route closure a client
inference.

## Decision

The next free successor bundle uses operation-registry format 2. It declares top-level
`api_majors: [1, 2]`, an `api_major` on every operation, and both majors in compatibility metadata.
It adds the five served v2 file/tree operations and uses their existing typed inputs and results.
V2 success, error, refusal and replay envelopes say `api_version: "v2"`; v1 routes retain their
bytes.

The registry declares the catch-all path syntax the router serves. A new versioned renderer handles
that syntax; the renderer named and hashed by every released bundle remains unchanged. Compatibility
requires the adjacent released predecessor and follows the complete resolved `$ref` closure when it
compares a preserved branch.

## Consequences

One bundle describes every route the daemon answers and clients can select by explicit major. The
daemon continues to advertise `substrate-wire/0.4.0` until a separate consumer-coordinated header
change; the successor remains a development bundle.
