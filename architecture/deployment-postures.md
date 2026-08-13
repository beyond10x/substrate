# Deployment postures

The contract is posture-independent. Postures change reachability, trust bootstrap, storage, and
operations—not resource semantics.

| Posture | Reachability | Authentication | Placement owner |
|---|---|---|---|
| personal | loopback by default | local configured token | caller/operator |
| organization | LAN or private overlay, explicit opt-in | per-service tokens; later identity-issued service material | organization control plane or caller |
| hosted | private workload network; no unauthenticated public listener | identity/cloud-managed service trust | cloud |
| satellite-adjacent | local to private connectors and endpoints; outward control relationship | deployment identity plus operation-scoped channel authority | cloud/connectors federation |

## Rules shared by every posture

- Binding a reachable address without authentication is refused at startup.
- The daemon governs exactly one machine or handed-over cluster scope.
- Capabilities report what that deployment verified; posture names never imply capability.
- No posture silently weakens a requested sandbox.
- Fleet scheduling, organization membership, billing, connector grants, and product quotas remain
  outside substrate.
- A direct local client may bypass connectors as a network hop, but never bypasses substrate's own
  authentication, limits, or enforcement.

Hosted and satellite trust details remain design work in
[authentication, secrets, and trust](../docs/design/06-authentication-secrets-and-trust.md).
