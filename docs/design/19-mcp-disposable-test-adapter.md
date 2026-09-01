# Design 19: MCP owns one disposable SDK-managed daemon

**Status:** accepted as [ADR 0025](../../adr/0025-the-mcp-adapter-is-a-disposable-test-surface.md) ·
**Date:** 2026-09-01

This design closes `story:mcp-managed-disposable-daemon`. It depends on the promoted-contract SDK
parity story; its implementation must not copy unfinished wire types into the adapter.

## Boundary and package

`crates/substrate-mcp` is `publish = false`, forbids unsafe code and produces `substrate-mcp`. Its
only direct Substrate dependency is the exact-version SDK with `linked-daemon`. It calls the SDK's
child re-exec entry point before normal startup, then creates a private temporary managed daemon with
one fresh deployment id. No host, store, daemon or wire type crosses this crate boundary.

An xtask gate proves the package remains private, the dependency edge is SDK-only, the MCP feature
set contains no HTTP, OAuth or client stack, and the implementation does not select an unbounded
stdio convenience transport. The MCP dependency is exact-version pinned and the licence inventory
is regenerated when it lands.

## Transport limits

The adapter implements a bounded newline-delimited JSON-RPC transport on stdin/stdout:

- 2,097,152 bytes maximum per incoming or outgoing frame;
- 128 bytes maximum for a string request id;
- 16 concurrent calls;
- 30 seconds maximum for an adapter-owned wait;
- stdout carries protocol frames only; bounded diagnostics use stderr;
- arguments, file bytes, environment values, output and credentials are never diagnostic fields.

An oversized, invalid-UTF-8 or malformed frame closes the session after bounded work. Batch requests
and every MCP capability not named below are absent.

## Closed surface

Server capabilities are `tools` and `resources` only. The tools are:

| tool | exact purpose |
|---|---|
| `machine_get` | advertised contract and capability facts, preserving absent facts |
| `workspace_create`, `workspace_get`, `workspace_destroy` | one empty bounded workspace lifecycle |
| `workspace_file_read`, `workspace_file_write`, `workspace_file_delete` | offset/limit bytes and explicit mutation ids |
| `workspace_lease_renew` | explicit-id lease renewal |
| `exec_start`, `exec_get`, `exec_wait` | argv-only bounded execution and observation |
| `exec_output_read` | bounded stdout/stderr page |
| `exec_signal`, `exec_lease_renew`, `exec_retire` | explicit-id lifecycle mutations |
| `operation_get` | durable reconciliation after an ambiguous transport result |
| `metrics_get` | exact requested exec or workspace measurements |

Raw-pipe and PTY sessions, event/metrics streams, snapshots, bulk diffs, host roots, secret slots and
egress apertures are excluded. The last three are operator-bound authority and cannot become
model-selected host reach.

Every mutation requires a caller ULID and identical replay keeps that id. Adapter validation uses a
`substrate-mcp.*` error namespace. A daemon refusal is an error result but retains the daemon's exact
class, code, safe message, address, retriable flag and operation id. Read and mutation output is one
canonical structured JSON envelope, also copied into text content for clients that cannot consume
structured output.

Static resources list only `substrate://machine`. Resource templates cover workspaces, percent-
encoded single-component file paths, execs, bounded output, operations and metrics. Traversal is
rejected both at URI decoding and again by the SDK.

## Lifecycle

Startup creates owner-private temporary state, starts the linked daemon child over its liveness pipe,
verifies the exact advertised contract and capability snapshot, and only then completes MCP
initialization. One adapter owns one subject, resource registry and exclusive process-free delegated
cgroup root. Because daemon startup reconciles every Substrate exec group under its configured root,
sharing a root between adapters is refused.

On EOF, SIGINT or SIGTERM the adapter stops new calls, cancels waits, sends KILL to each tracked active
exec, polls it terminal, retires it, destroys workspaces, closes the liveness pipe, terminates and
reaps the daemon, and removes temporary state only after child absence is proved. A cleanup timeout or
kill/wait error exits nonzero and retains state. Forced parent death must still terminate the daemon
and workloads through the liveness relationship; leftover named filesystem state after SIGKILL is an
explicitly unclaimed property.

The SDK prerequisite fixes `ManagedDaemon::shutdown` and startup cleanup so moving the child into a
fallible termination routine cannot cause the temporary root to drop on an unknown outcome.

## Image and evidence

`Dockerfile.mcp` builds one distroless nonroot binary and declares no port or volume. The release
publishes it write-once and keyless-signs/verifies its digest. Its mandatory container smoke uses a
read-only root, private tmpfs and `--network=none`, then proves the portable named confinement refusal
and complete container removal. It never treats missing delegated confinement as a positive pass.

The deterministic gate snapshots the tool/resource schemas, bounds, annotations and unsupported
capabilities; tests every frame and concurrency limit; proves operation-id replay and ambiguous-result
reconciliation; projects every refusal class; checks URI/path/base64 pagination; and injects child
kill/wait failures. A shipped-binary stdio journey covers machine, workspace, file, exec, output,
metrics, retire and destroy. The delegated lane runs a real bounded `sha256sum`, verifies exact output,
applied confinement and usage, and leaves no process or cgroup. EOF, signals, cancellation, startup
failure and abrupt parent death each prove the lifecycle claim they actually make.

`cargo xtask mcp-codex-smoke --server target/release/substrate-mcp` is a manual credentialed smoke,
not the sole gate. It uses an ephemeral Codex run, pre-generated operation ids and the same portable
refusal/delegated-success split. No Codex or API credential enters the repository or CI.
