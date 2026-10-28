---
format: aep.planning-md/3
id: story:adapter-live-output
kind: story
status: draft
title: A confinement adapter delivers a child's output while it runs
relations:
- decomposes: epic:confinement-contract-family
revision: 1
---
# A confinement adapter delivers a child's output while it runs

## Consumer ask

A consumer launching children through the confinement adapters (loom,
`story:child-launches-through-confinement-adapters`) needs output as it is produced. Today the
adapters return output only after the run ends.

## Design

[Design 32](../../../docs/design/32-adapter-stdin-and-live-output.md) D3 to D9, D11:
`OutputDelivery::Live { queued_chunks }` (at most 16), `read_output(run, timeout)` returning
`Chunk { stream, offset, bytes }`, `Bounded`, `Stalled` or `Interrupted`, the `output_live` fact,
the 1 s stall deadline that kills the run, and the post-run record equal to the delivered head.

## Acceptance

- An adapter that declares `output_live` delivers each stream in order with contiguous offsets, and
  any loss is a named event; nothing is dropped silently.
- A reader that stops reading past the stall deadline gets `Stalled` and the run is killed.
- `Live` with `Tail` or `HeadAndTail` retention is refused
  `confinement.live-output-retention-invalid` (422) before anything runs.
- An adapter without the fact refuses `Live` `confinement.fact-unserved` naming `output_live`.
- Wave 1 (`local`, `linux`, `systemd`) lands with its ESS confinement scenarios; `macos`, `oci` and
  then `pod` (`pods/log?follow=true`) follow, each with its lane case green.
- Each new case runs once; the stall case costs one 1 s deadline; no new CI job.
