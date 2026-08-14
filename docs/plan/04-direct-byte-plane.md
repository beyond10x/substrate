# Plan 04: direct byte plane

**Status:** active · **Date:** 2026-08-14

This phase adds leased interactive process channels without weakening the phase-3 workspace,
operation, capability-snapshot, recovery, and event guarantees. The first consumer pressure is the
model-free governed-harness slice accepted by
[architecture ADR 0023](https://github.com/daemonloom/architecture/blob/main/adr/0023-governed-harness-execution-is-defense-in-depth.md).

## Slice A: raw-pipe development contract

**Progress:** the deterministic `substrate-wire` 0.4.0 development bundle has 26 closed operations,
21 executable vectors, 71 design vectors, 112 checked requirements, and 11 hash fixtures. Its
session vocabulary implements
[ADR 0008](../../adr/0008-pipe-sessions-have-distinct-durable-identity.md), and Agent consumes an
exact independently verified copy without a Substrate source dependency. Packaging, signing, and a
stable clean-room release remain open.

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
retains a bounded attachment tombstone. Portable refusal and semantic route tests pass. The
distinct session resource is implemented and terminal/restart projections are atomic. The real
delegated Agent lane proves empty environment, no egress, applied bounds, pressure, attachment and
protocol-loss containment, lease expiry, restart reconciliation, and whole-tree cleanup.

- reuse the proven workspace, bubblewrap, cgroup, empty environment, output draining, lease, and
  whole-tree cancellation implementation;
- expose one owner-permissioned Unix-socket channel with bounded frames and queues;
- refuse when delegated confinement or the no-egress posture is unavailable;
- retain stdout and stderr separately and keep protocol bytes out of diagnostics;
- make the durable operation/session terminal state authoritative before cleanup/reclamation.

## Slice C: Agent compatibility proof

**Progress:** Agent's no-fallback backend consumes the exact copied 0.4.0 bundle and drives the
model-free fake Codex app-server through a real delegated daemon. It checks capability/profile/
configuration binding, bidirectional bytes, bounded pressure, approval lifecycle, half-close,
attachment and protocol failure, lease expiry, restart reconciliation, exact terminal pairing, and
whole-tree cleanup. No model, credential, egress, public CLI surface, or sibling source dependency
is involved. Stable/public `substrate_confined` reporting remains an Agent release gate.

- run a synthetic app-server protocol process with no model, credential, or network;
- prove start ordering, bidirectional framing, output pressure, cancellation, child-tree cleanup,
  lease expiry, and terminal observation;
- let the consumer use copied development contract bytes or a separately built artifact, never a
  sibling Substrate source dependency;
- report `substrate-confined` only from Substrate capability and terminal observations.

## Slice D: immutable execution capsule

**Progress:** complete for the bounded inline development capsule.
[ADR 0009](../../adr/0009-execution-capsules-are-verified-read-only-inputs.md) fixes a bounded inline
development contract: Substrate independently verifies exact application, configuration, sidecar,
and hook bytes, mounts them read-only at `/runtime`, keeps `/workspace` mutable and separate, and
reports the applied capsule identity. Agent compiles that identity from its profile and rejects
drift before model dispatch. The deterministic 0.4.0 bundle has 26 closed operations, 21 executable
vectors, 71 design vectors, 112 checked requirements, and 11 exact hash fixtures. Normal completion
owns capsule cleanup through terminal tree reconciliation; startup removes stale private capsule
directories only after orphan cgroups are reconciled and refuses unexpected or symlink entries.

- publish a deterministic successor bundle with canonical capsule hashing and adversarial vectors;
- materialize only bounded regular files from validated relative paths and retain the private
  directory through whole-tree terminal reconciliation;
- prove digest/path/entrypoint refusal, read-only enforcement, workspace separation, and cleanup;
- extend the real model-free Agent lane so the fake app-server and hook/config fixtures execute
  from the applied capsule;
- keep host base closure, signing, registry/OCI transport, secrets, egress, and public reporting as
  explicit later gates.

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
