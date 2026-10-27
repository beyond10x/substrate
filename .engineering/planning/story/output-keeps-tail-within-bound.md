---
format: aep.planning-md/3
id: story:output-keeps-tail-within-bound
kind: story
status: active
title: Output keeps the tail of each stream, or head and tail, within the same bound
tags:
- adr-0109
relations:
- decomposes: epic:confinement-contract-family
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-10-27T13:13:28Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-10-27T13:13:28Z", actor: "human:timo", revision: 3}
---
## Outcome

A consumer chooses, per output stream, whether a run keeps the head, the tail, or the head and
tail of its output, within the same byte bound.

## Decision

Session b10x-chef-orchestrator, decision D88 (2026-10-27): ships in the same release as
`story:linux-adapter-serves-scoped-workspace-write`, under the same design note. Reason given: a
test run that fails with more than 1 MiB of output loses its `test result:` line and the failing
test names, because only the head is kept; a consumer's former runner kept the tail, so moving it
onto substrate regresses it. That consumer keeps two adversary cases ignored under this story's id
until the release.

## Evidence (read at a62a2960)

- `crates/substrate-host/src/confinement/local.rs:44-45`: `LOCAL_OUTPUT_LIMIT_BYTES = 1_048_576`,
  "Each output stream is kept up to this many bytes and drained past it" (head only).
- `crates/substrate-host/src/confinement/linux.rs:238`: `linux` takes the driver's
  `output_limit_bytes`, also head only.

## Acceptance

1. A consumer chooses head, tail or head and tail per stream through the consumer surface; the
   change is additive to the frozen surface and the CHANGELOG states it.
2. The byte bound stays the same; the kept bytes never exceed it.
3. The default for existing consumers is unchanged unless the CHANGELOG says otherwise.
4. One case per mode, each run once; adversary pass, and a security review where confinement code
   changes, every finding recorded with its outcome.

## Design

Changes a capability of the consumer surface. The design is
[`docs/design/29-scoped-workspace-write-and-output-tail.md`](../../../docs/design/29-scoped-workspace-write-and-output-tail.md),
§§ 5–8; it needs no ADR (the note says why). `pod` refuses the tail modes by name.
