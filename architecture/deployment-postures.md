# Deployment postures

The contract is posture-independent. Postures change reachability, trust bootstrap, storage, and
operations—not resource semantics.

| Posture | Reachability | Authentication | Placement owner |
|---|---|---|---|
| personal | owner-permissioned Unix socket or loopback TCP by explicit configuration | OS peer identity or expiring generated bearer; never unauthenticated | caller/operator |
| organization | LAN or private overlay, explicit opt-in | per-service token or identity-issued material | organization control plane or caller |
| hosted | private workload network; no unauthenticated public listener | identity/cloud-managed service trust | cloud |
| satellite-adjacent | local to private connectors and endpoints; outward control relationship | deployment identity plus operation-scoped channel authority | cloud/connectors federation |

## Rules shared by every posture

- Binding a reachable address without authentication is refused at startup.
- A non-loopback control listener additionally requires TLS/mTLS or a configured trusted tunnel.
- The daemon governs exactly one machine or handed-over cluster scope.
- One daemon is one trust domain and, for v1, one tenant; hosted placement never multiplexes
  mutually untrusted tenants through it.
- Capabilities report what that deployment verified; posture names never imply capability.
- No posture silently weakens a requested sandbox.
- Fleet scheduling, organization membership, billing, connector grants, and product quotas remain
  outside substrate.
- A direct local client may bypass connectors as a network hop, but never bypasses substrate's own
  authentication, limits, or enforcement. Architecture ADR 0013 limits this to the personal trust
  domain and records that connector grants and platform audit are absent.

Hosted trust follows
[architecture ADR 0015 — Foundation services share one trust envelope](https://github.com/daemonloom/daemonloom/blob/e01ea676da18fb855814e7621514e0c98fc57c2c/architecture/adr/0015-foundation-trust-envelope.md);
satellite federation follows
[architecture ADR 0018 — Connectors satellites federate outward under bounded authority](https://github.com/daemonloom/daemonloom/blob/e01ea676da18fb855814e7621514e0c98fc57c2c/architecture/adr/0018-connectors-satellite-federation.md).
Substrate-specific handling is fixed in
[authentication, secrets, and trust](../docs/design/06-authentication-secrets-and-trust.md).
