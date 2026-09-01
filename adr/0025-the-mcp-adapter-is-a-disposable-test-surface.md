---
status: accepted
date: 2026-09-01
---

# ADR 0025: the MCP adapter is a disposable test surface

## Context

Arbitrary harnesses need a convenient way to exercise Substrate without embedding its daemon or
learning its private host-driver APIs. MCP is useful for that purpose, but making the daemon itself
an MCP server would add a second production ingress, blur its wire contract, and tempt callers to
bypass durable operation identity and named refusals.

The Rust SDK already owns the permitted client boundary and may link the daemon only to re-exec it
as a separate child (ADR 0022). A disposable adapter can compose those pieces while keeping the
daemon process, Unix socket and confinement checks real.

## Decision

Substrate provides a private, non-publishable Rust workspace crate whose binary starts one fresh
managed daemon, verifies its advertised contract and capability snapshot, serves a closed MCP
surface over stdin/stdout, and tears down every resource it created on orderly exit. It reaches
Substrate only through the public Rust SDK and Unix-socket wire; it has no daemon, host, store or
wire implementation dependency.

MCP is stdio-only. It exposes bounded workspace, file, exec, operation, output and metrics tools and
read resources. Every mutation requires a caller-supplied operation id, and every daemon refusal is
projected without changing its class, code, retriable flag, address or operation id. Model-selected
host roots, secret slots, egress apertures, daemon paths, cgroup roots and TCP configuration are not
part of the surface.

The adapter is development and conformance tooling, not production ingress. It authenticates no
remote principal, opens no network listener, supplies no policy defaults and claims no MCP stability
independent of its matching Substrate release. Adding HTTP MCP transport, OAuth, any crate-registry
publication or product policy requires a successor decision.

The binary also ships as a separate, keyless-signed public image:
`ghcr.io/beyond10x/b10x-substrate-mcp:<substrate-version>`. It has no exposed port or volume and is
intended for `--network=none`, stdin attached, a read-only root and a private writable tmpfs. The
portable container lane must prove the exact named sandbox-unavailable refusal; it must not claim
positive confined execution when bubblewrap, user namespaces or an exclusive delegated cgroup are
absent. Positive execution remains a native delegated-lane claim until a separate container-runtime
design provides and verifies those prerequisites. Image publication is write-once, signed by digest
and verified before announcement under the same release discipline as the daemon.

Orderly EOF, SIGINT and SIGTERM cleanup is a hard claim. The adapter stops admission, cancels bounded
waits, kills and terminally observes active execs, retires them, destroys their workspaces, then
stops and reaps the daemon before deleting temporary state. Unknown child absence retains state and
returns failure. Abrupt SIGKILL cannot guarantee filesystem deletion and is not described as if it
could; it must still prove through the liveness pipe that no daemon or workload process survives.

## Consequences

Codex and other harnesses get one command and one image for realistic local Substrate testing while
all actual operations still cross the public daemon contract. MCP does not become a driver contract,
an authentication mechanism or an alternative durability model.

The SDK must first expose caller operation ids, bounded output, metrics and absence-preserving facts,
and its managed-daemon cleanup must retain state until child absence is proved. The adapter needs a
bounded MCP codec and additional dependency, licence and image gates. Its public image increases the
release workflow's signed artifact set but does not make the MCP surface stable.
