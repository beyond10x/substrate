---
format: aep.planning-md/3
id: story:adapter-live-output
kind: story
status: draft
title: A confinement adapter delivers a child's output while it runs
relations:
- decomposes: epic:confinement-contract-family
- depends_on: story:adapter-io-spec
revision: 2
---
# A confinement adapter delivers a child's output while it runs

## Consumer ask

A consumer launching children through the confinement adapters needs output as it is produced.
Today the adapters return output only after the run ends.

## Design

[Design 32](../../../docs/design/32-adapter-stdin-and-live-output.md) D3 to D10, D14:
`OutputDelivery::Live { queued_chunks }` (at most 16), `read_output(run, timeout)` returning
`Chunk { stream, offset, bytes }`, `Bounded` or `Stalled`, the loss events synthesized from the
drain's flags after the queue, `output_stalled(run)`, the `output_live` fact, the 1 s stall
deadline that kills the run, and the post-run record that begins with the delivered head.

## Depends on

`story:adapter-io-spec`, which makes every spec change of the design first. This story edits no
scenario and not the suite (ADR 0044 decision 6).

## Acceptance

- An adapter that declares `output_live` delivers each stream in order with contiguous offsets, and
  any loss is a named event after every chunk; nothing is dropped silently.
- A reader that stops reading past the stall deadline gets `Stalled`, the run is killed, and
  `output_stalled` is true for it.
- The record begins with the delivered bytes, equals them unless the run stalled, and carries no
  truncation marker under `Live`.
- `Live` with `Tail` or `HeadAndTail` retention is refused
  `confinement.live-output-retention-invalid` (422) before anything runs.
- An adapter without the fact refuses `Live` `confinement.fact-unserved` naming `output_live`;
  `pod` is one of them until a design states merged-stream delivery.
- Served on `local`, `linux`, `systemd` around both, `macos` and `oci`; the lanes the spec story
  turned red are green on the integration branch once this story and `story:adapter-stdin-pipe`
  have landed.
- Each new case runs once; a stall case costs one 1 s deadline; no new CI job.
