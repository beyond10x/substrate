# Plan 04: direct byte plane

**Status:** active · **Date:** 2026-08-14

This phase adds leased interactive process channels without weakening the phase-3 workspace,
operation, capability-snapshot, recovery, and event guarantees. The first consumer pressure is the
model-free governed-harness slice accepted by
[architecture ADR 0023](https://github.com/daemonloom/architecture/blob/main/adr/0023-governed-harness-execution-is-defense-in-depth.md).

## Slice A: raw-pipe development contract

**Progress:** source-typed start/client/server frame vocabulary exists under
`substrate-wire`, with a development contract note. Deterministic successor bundle generation,
adversarial vectors, and the clean-room runner remain open.

- add a successor development bundle; never rewrite immutable 0.1.0 bytes;
- define session resource, start/get/cancel/retire operations, capability facts, and closed channel
  frames for stdin/stdout/stderr, close-input, signal, exit, truncation, and protocol error;
- bind session, exec, workspace, operation, subject, capability/configuration snapshot, and lease;
- define single attachment, half-close, backpressure, disconnect, expiry, replay, and terminal
  reconciliation behavior;
- provide positive and adversarial vectors plus a clean-room runner protocol.

## Slice B: confined Linux pipe session

**Progress:** the host crate can start, write, read, half-close, bound, and cancel a pipe process by
reusing the exec confinement path; portable tests prove refusal without hard confinement and stream
attribution. Durable session storage, lease/attachment ownership, daemon routing, and delegated
whole-tree integration evidence remain open.

- reuse the proven workspace, bubblewrap, cgroup, empty environment, output draining, lease, and
  whole-tree cancellation implementation;
- expose one owner-permissioned Unix-socket channel with bounded frames and queues;
- refuse when delegated confinement or the no-egress posture is unavailable;
- retain stdout and stderr separately and keep protocol bytes out of diagnostics;
- make the durable operation/session terminal state authoritative before cleanup/reclamation.

## Slice C: Agent compatibility proof

- run a synthetic app-server protocol process with no model, credential, or network;
- prove start ordering, bidirectional framing, output pressure, cancellation, child-tree cleanup,
  lease expiry, and terminal observation;
- let the consumer use copied development contract bytes or a separately built artifact, never a
  sibling Substrate source dependency;
- report `substrate-confined` only from Substrate capability and terminal observations.

## Later phase-4 slices

PTY/resize behavior, network WebSocket/TLS transport, proof-bound brokered authority, reconnect,
sealed named secret slots, and destination-bound egress remain separate additions. A live vendor
harness is refused until the required secret and egress capabilities exist.

## Exit evidence

- all existing 0.1.0/0.2.0 gates remain byte-clean and green;
- the successor bundle is deterministic, fully classified, and independently consumable;
- portable hosts refuse pipe sessions without confinement;
- the delegated Linux lane proves bounded bidirectional bytes and whole-tree terminal cleanup;
- the model-free Agent fixture passes without importing Agent or vendor code into Substrate.
