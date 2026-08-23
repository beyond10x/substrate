---
date: 2026-08-13
status: accepted
---

# ADR 0002: substrate owns execution, not product policy

## Context

Connectors owns grants and vendor integration, cloud owns hosted composition and placement, agent
products own execution loops, and autodev owns software-factory scheduling. Substrate still needs
strong local admission and enforcement to protect the machine it governs. Combining these concerns
would make the data plane a second control plane and fragment policy across services.

## Decision

Substrate owns generic resource lifecycles, coarse local authentication, capability admission,
limits, isolation, driver execution, leases, operation reconciliation, and observed state for one
machine or handed-over cluster scope. It does not own organizations, rich grants, provider/vendor
semantics, agent loops, harnesses, fleet scheduling, billing, or product workflows.

Higher layers may deny a substrate action before calling it. They cannot weaken substrate's own
guards, and their permit is never treated as proof that local enforcement succeeded.

## Consequences

The direct API remains useful without the rest of the b10x stack. Connectors can govern it as a
first-party provider, and cloud can compose fleets, without either becoming required runtime
dependencies. Some requests are admitted twice for different reasons: rich intent above and machine
safety inside substrate.
