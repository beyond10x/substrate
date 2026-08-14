# Plan 04: direct byte plane

**Status:** active · **Date:** 2026-08-14

This phase adds leased interactive process channels without weakening the phase-3 workspace,
operation, capability-snapshot, recovery, and event guarantees. The first consumer pressure is the
model-free governed-harness slice accepted by
[architecture ADR 0023](https://github.com/daemonloom/architecture/blob/main/adr/0023-governed-harness-execution-is-defense-in-depth.md).

## Slice A: raw-pipe development contract

**Progress:** source-typed capability/start/client/server vocabulary and an independently
digest-pinned Agent copy exist. Deterministic successor bundle generation, full adversarial vectors,
and the clean-room released-bundle runner remain open.

- add a successor development bundle; never rewrite immutable 0.1.0 bytes;
- define session resource, start/get/cancel/retire operations, capability facts, and closed channel
  frames for stdin/stdout/stderr, close-input, signal, exit, truncation, and protocol error;
- bind session, exec, workspace, operation, subject, capability/configuration snapshot, and lease;
- define single attachment, half-close, backpressure, disconnect, expiry, replay, and terminal
  reconciliation behavior;
- provide positive and adversarial vectors plus a clean-room runner protocol.

## Slice B: confined Linux pipe session

**Progress:** the host crate can start, write, read, half-close, bound, and cancel a pipe process by
reusing the exec confinement path. The daemon durably reserves the leased underlying exec before
dispatch and serves one subject-scoped Unix-WebSocket attachment with strict ordering, bounded
messages/control rate/lifetime, cancellation on loss, separate stderr, and authoritative terminal
persistence. Live forwarding stops at the admitted output bound, and loss containment closes the
output queue before kill so backpressure cannot block reconciliation; an uncertain containment
retains a bounded attachment tombstone. Portable refusal and semantic route tests pass; a distinct session identity,
successor bundle, and delegated whole-tree integration evidence remain open.

- reuse the proven workspace, bubblewrap, cgroup, empty environment, output draining, lease, and
  whole-tree cancellation implementation;
- expose one owner-permissioned Unix-socket channel with bounded frames and queues;
- refuse when delegated confinement or the no-egress posture is unavailable;
- retain stdout and stderr separately and keep protocol bytes out of diagnostics;
- make the durable operation/session terminal state authoritative before cleanup/reclamation.

## Slice C: Agent compatibility proof

**Progress:** Agent has a no-fallback copied-contract backend and a clean-room semantic server which
drives the model-free fake Codex app-server through the execution port. It pins the copied contract
digest and checks inspection, applied confinement shape, bidirectional bytes, approval lifecycle,
half-close, and terminal reconciliation. This is semantic compatibility, not delegated
`substrate-confined` evidence.

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
