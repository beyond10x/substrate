# Domain model

The model separates desired commands from observed resources. Requests describe bounded actions;
responses describe what the driver could prove afterward.

## Resource families

| Family | Identity and lifecycle | Core observation |
|---|---|---|
| machine | one daemon scope | verified drivers, enforcement capabilities, limits, versions |
| workspace | server-minted id, optional lease | confined root, source revision, size and lifecycle state |
| exec | server-minted id plus caller operation id | applied sandbox, process state, exit, bounded output |
| session | leased exec-side channel | authority state, attachment count, terminal outcome |
| workload | image-backed long-lived unit | desired and observed state, restarts, last exit |
| image | digest identity | available digest, provenance facts, build/pull observation |
| volume | server-minted id | attachment and storage observations |
| endpoint | server-minted id | applied exposure and reachable address |
| operation | caller-minted id | unseen, accepted/in-flight, or terminal outcome |

## Invariants

- Resource identifiers are opaque and server-minted; operation identifiers are caller-minted.
- A mutation with the same operation id and same body returns the same logical outcome. Reuse with a
  different body is a conflict.
- Unknown observed data remains unknown; it is never rendered as success, zero, or absence.
- Requested isolation and applied isolation are distinct fields.
- Every optional feature is gated by a verified capability fact.
- Destruction, lease expiry, cancellation, and failure are typed transitions and emit events.
- Driver-specific facts may enrich capability data but cannot leak driver-specific command shapes
  into the common contract.

The detailed operation families and risk metadata live in the
[API contract](../docs/design/01-contract.md).
