# Design 18: a Rust SDK remains a wire client

**Status:** accepted as [ADR 0022](../../adr/0022-the-rust-sdk-remains-a-wire-client.md), with
source distribution amended by
[ADR 0030](../../adr/0030-rust-crates-are-source-distributed-and-non-publishable.md) ·
**Date:** 2026-08-31

## Problem

Substrate already has a closed Rust representation of its wire and a daemon which creates,
controls and observes confined resources, but a Rust consumer has to assemble HTTP, JSON,
operation ids, recovery and WebSocket handling itself. The low-level host `Driver` looks reusable
but calling it directly skips peer authentication, durable-before-dispatch operations, leases,
events and recovery. That is precisely the optimistic execution path Substrate exists to remove.

The SDK must also support a single-file application distribution without turning the daemon into
an in-process library call. The architecture permits a composition root to link the daemon only so
it can re-execute a private child; every resource operation still crosses the Unix socket.

## Decisions

### One high-level client over the advertised contract

The source-distributed `b10x-substrate-sdk` connects to an owner-private Unix socket, reads
`GET /v1/machine`, and verifies the `substrate-wire/0.4.0` contract name and digest before it serves
requests. Its public surface is SDK-owned `Machine`, `Workspace`, `Exec`, `RunOutput`,
`PipeSession`, `Event`, `Operation` and error types plus builders and resource handles. It does not
re-export the implementation's frontier wire types: those already describe development additions
which the daemon does not advertise.

Workspace creation is empty-only because that is what the host currently serves. File operations
remain bounded. A command is argv-only and has no shell-string escape hatch. Its builder requires
an explicit execution policy containing wall time, cumulative CPU, memory-plus-swap, process count
and captured-output bounds. Substrate supplies construction ergonomics, never product policy
defaults. The SDK binds the current verified capability snapshot and always requests the required
workspace confinement profile with no network for this first surface.

Every mutation receives one caller-minted ULID unless the caller supplied one. If transport fails
without an answer, the client queries the operation ledger. It may replay the byte-identical body
once under the same id when the ledger says it is absent; it never creates a replacement id. A
second unanswered result becomes an `UnknownOperation` carrying the reconciliation handle.
Refusals retain their canonical class, code, address, retriable fact and operation id.

Event streaming exposes the daemon cursor. A retention or source gap is a typed gap and requires a
reconciliation snapshot; the SDK never silently starts at the newest event. Raw-pipe sessions keep
their single attachment and bounded closed frame vocabulary. V2 workspace byte operations and PTY
mode stay absent from the public SDK until the daemon advertises their successor contract.

### Managed means a separately owned process

`ManagedDaemonBuilder` requires a caller-owned data directory and deployment id. It derives the
state database, workspace root and socket beneath that directory, admits only the invoking
effective uid, and either spawns a named `substrate-daemon` binary or re-executes the current
application into an opt-in linked-daemon entrypoint. Both paths wait for the same verified machine
response and return the same `Client`.

The child has a parent-liveness pipe. Explicit shutdown or dropping the owner closes it; the daemon
stops gracefully, then the supervisor force-kills and reaps it after a bounded grace period. State
and workspaces remain. A separate explicit temporary mode owns and removes its data directory only
after the child is gone. Startup diagnostics are byte-bounded.

The SDK never constructs `App`, calls `Driver`, or handles a resource request in the parent
process. The linked feature may call the daemon composition entrypoint only after re-exec. This
preserves kernel peer credentials, independent failure, socket permissions and one behavior in
external and linked deployments.

### Runtime crates are source-distributed and non-publishable

The source package names are `b10x-substrate-wire`, `b10x-substrate-store`,
`b10x-substrate-host`, `b10x-substrate-daemon` and `b10x-substrate-sdk`. Runtime dependencies use
the exact workspace release version, and every workspace package sets `publish = false`. The gate
checks the complete non-publishable set plus the runtime packages' fixed names, exact internal
edges, SPDX metadata, READMEs and public documentation targets.

Consumers use a local path or an exact Git revision. Tagged releases publish signed GitHub and
GHCR artifacts but no crate registry upload, registry credential or docs.rs page. This distribution
choice does not make any wire bundle stable.

## Failure handling

- Contract identity or digest mismatch fails connection before a resource method is available.
- A stale capability snapshot is returned as the daemon's named refusal; the SDK does not hide it
  behind a fresh operation id.
- An early child exit, readiness timeout, occupied state identity or malformed linked-child
  bootstrap document is a typed startup failure with bounded diagnostics.
- Owner loss closes the liveness pipe. A child that ignores graceful shutdown is killed and
  reaped; it is never detached as an accidental service.
- `Drop` initiates shutdown, while `shutdown().await` is the API that reports its result.

## Compatibility

This design adds no route, request field, response field, event, capability or refusal. Frozen
contract bundles do not move and the daemon continues to advertise `substrate-wire/0.4.0`.
Source distribution changes Rust package metadata and public Rust APIs only. The SDK is versioned
below 1.0 and pins the exact runtime chain used by linked mode.
