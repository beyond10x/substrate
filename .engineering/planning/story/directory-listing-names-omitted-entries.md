---
format: aep.planning-md/3
id: story:directory-listing-names-omitted-entries
kind: story
status: draft
title: Directory listings name the entries they omit
relations:
- decomposes: epic:review-2026-09-03-findings
revision: 1
---
# Story: Directory listings name the entries they omit

## Context

story:directory-listing-survives-special-entries makes the v1 directory page and the v2 tree omit
entries the wire cannot represent (FIFOs, sockets, devices, non-UTF-8 names) instead of refusing.
Without a successor bundle it can only signal that on v2, by reusing `truncated`, and on v1 not at
all; it records both as accepted trade-offs against invariant 3.

## Acceptance

A successor wire bundle publishes a machine-readable omission signal on both the v1 directory page
and the v2 tree, distinct from `truncated`, and a runtime vector shows a directory with a FIFO
answering it on both routes.

## Notes

Needs an ADR or design document before code (invariant 8) and a successor bundle (invariant 6).
Left draft until the next bundle is cut.
