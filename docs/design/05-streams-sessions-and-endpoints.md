# Design 05: streams, sessions, and endpoints

**Status:** accepted phase-4 design · **Date:** 2026-08-13

Substrate has semantic streams and continuous byte channels. They share authentication and resource
lifecycle but must not be collapsed into one unbounded WebSocket convention.

## 1. Semantic streams

Event streams, build progress, log tails, and captured exec output use closed frame sets with:

- resource and operation provenance;
- monotonically ordered cursor or offset within a documented scope;
- explicit end, truncation, cancellation, and error frames;
- byte and retention bounds;
- reconnect behavior that states whether replay is available.

These streams may be relayed through connectors because their contents remain bounded semantic
events.

## 2. Sessions

An interactive exec creates a leased session resource before opening its channel. Establishment
authenticates the control request, admits scopes/capabilities, creates the bounded process, and mints
or accepts a short-lived channel authority. The mode is explicit:

- `pty` is a human terminal and has input, output, resize, signal, exit, and protocol-error frames;
- `pipes` is a machine protocol and preserves distinct stdin, stdout, and stderr with input,
  output, close-input, signal/cancel, exit, truncation, and protocol-error frames.

A PTY is never substituted for pipes. Both modes have finite frame, aggregate-byte, queue, idle,
lease, and attachment limits. Backpressure or attachment loss follows typed cancellation or
reconciliation behavior rather than unbounded buffering.

Session authority is operation-scoped, short-lived, revocable, non-loggable, proof-bound on network
transport, and single-use for initial redemption. Reconnection requires a fresh authority;
attachment cannot imply an unbounded process lease.

## 3. Endpoints and tunnels

An endpoint exposes an observed address for a workload or exec port under an explicit exposure
class. Loopback and LAN exposure are distinct. Public ingress, DNS, TLS termination, and arbitrary
reverse tunnels are not substrate v1 responsibilities.

A tunnel is a direct byte channel to an admitted endpoint. It carries no generic framing language
and cannot select a different destination after establishment.

## 4. Connectors byte-plane integration

Connectors may authorize session establishment under its rich grants and return an operation-scoped
authority. Continuous terminal or tunnel bytes then flow directly between client and substrate;
they do not traverse ordinary connector invocation or event delivery.

The authority wire must work for direct, hosted, and satellite-adjacent deployments without giving
substrate tenant policy or giving connectors access to daemon internals.

## Phase-4 decisions

1. **Endpoint reference:** the broker returns `{uri, transport, deployment, resource, operation,
   channel_kind, authority, authority_expires_at, session_lease_expires_at}`. Authority is a body
   field and is absent from URLs, logs, events, and durable client configuration.
2. **Issuer:** connectors mints governed authority after grant admission; substrate mints only for
   direct personal admission. Substrate verifies the accepted issuer and independently enforces
   local ownership/capability/lease, as fixed by
   [architecture ADR 0016 — Direct-byte establishment uses operation-scoped authority](../../../../architecture/adr/0016-operation-scoped-session-authority.md).
3. **Redemption:** network authorities are proof-bound, valid for at most 60 seconds, and redeem
   exactly once. Reconnect obtains a fresh authority. V1 permits one concurrent attachment.
4. **Transport:** WebSocket over TLS carries the v1 closed frames; an owner-permissioned Unix socket
   may carry the same frames locally. Semantics and authority remain transport-independent.
5. **Reachability:** substrate advertises only observed configured routes. There is no NAT traversal,
   public ingress, reverse tunnel, or connector byte relay in v1; no client-reachable route is
   `unserved`.

6. **Machine protocols:**
   [ADR 0007](../../adr/0007-protocol-processes-use-raw-pipe-sessions.md) fixes `pipes` as the first
   implementation slice. It is owner-permissioned Unix-socket, single-attachment, no-egress, and
   model-free. PTY, network transport, secret slots, and controlled egress remain absent until
   their own slice proves them.

The first phase-4 daemon slice is implemented: bounded raw stdin/stdout/stderr pipes reuse the
existing confinement path and refuse without delegated isolation; a durable leased exec reservation
precedes dispatch; and one subject-scoped Unix-WebSocket attachment enforces ordering, bounds,
half-close, output-bounded forwarding, backpressure-safe loss cancellation, and terminal
persistence.

[ADR 0008](../../adr/0008-pipe-sessions-have-distinct-durable-identity.md) closes the remaining
identity question: `ses_…` is a durable public resource bound one-to-one to its underlying `ex_…`;
the pair is reserved and terminalized atomically, attachment is claimed durably once, and one exec
lease drives both projections. That lifecycle, the deterministic 0.3.0 successor development
bundle, and delegated model-free Agent evidence are implemented. Stable release, network authority,
endpoints, tunnels, reconnect, and PTY remain open. Phase 4 is active under
[Plan 04](../plan/04-direct-byte-plane.md).
