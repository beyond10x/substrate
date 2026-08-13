# Design 05: streams, sessions, and endpoints

**Status:** draft for review · **Date:** 2026-08-13

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
or accepts a short-lived channel authority. PTY frames form a closed set: input, output, resize,
signal, exit, and protocol error.

Session authority is operation-scoped, short-lived, revocable, non-loggable, and preferably
single-use for initial redemption. Reconnection policy is explicit; attachment cannot imply an
unbounded process lease.

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

## Decisions required before implementation

1. Endpoint-reference format and binding to resource, operation, principal, and expiry.
2. Whether substrate mints authorities or verifies authorities minted by a trusted broker.
3. Single-use redemption, reconnect, and concurrent attachment rules.
4. WebSocket versus another transport for direct bytes; semantics must remain transport-independent.
5. NAT and private-network handling without embedding a reverse-tunnel product in substrate.
