---
status: accepted
date: 2026-08-13
---

# ADR 0003: v1 starts with one minimum host slice

## Context

The founding contract describes six eventual resource families across host, Docker, and Kubernetes.
Implementing that breadth before proving isolation, observation, and reconciliation would freeze
speculative driver and cross-foundation assumptions.

## Decision

The first implementation is the exact phase-2 endpoint set in Design 07, served by one selected
Linux host driver. It proves guarded empty workspaces, bounded file operations, argv-only execution,
observation, cancellation, operation replay/reconciliation, and machine capabilities.

Git/bundles, leases, event delivery, sessions, workloads, images, volumes, endpoints, Docker,
Kubernetes, connector projection, hosted identity, and fleet placement are absent and assigned to
later named phases. They may not appear as optimistic stubs or weaken the minimum contract.

## Consequences

- Implementation may begin after the design-closure record is accepted.
- The daemon remains useful and testable without another b10x repository.
- Later families enter only through capability-gated compatible contract releases.
