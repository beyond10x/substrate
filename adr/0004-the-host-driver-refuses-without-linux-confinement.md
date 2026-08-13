---
status: accepted
date: 2026-08-13
---

# ADR 0004: the host driver refuses without Linux confinement

## Context

Caller-controlled processes are host authority. A “best effort” sandbox or process-group-only
cleanup would turn missing kernel/backend support into silent arbitrary execution.

## Decision

The minimum exec capability is Linux-only and requires a dedicated unprivileged identity,
`openat2`-rooted file access, probed bubblewrap namespaces, a delegated cgroup v2 subtree with
whole-cgroup kill and resource bounds, cleared environment/descriptors, and a network namespace
with no egress. Missing enforcement removes the affected capability; there is no unconfined
fallback.

Personal Unix sockets authenticate OS peers. TCP requires an expiring generated bearer and TLS or
a trusted tunnel. Every resource and operation is deployment/subject scoped.

## Consequences

- Non-Linux hosts and Linux machines without the full backend can serve only capabilities they
  prove; they cannot claim minimum-host exec conformance.
- The contract states its kernel boundary honestly rather than claiming protection from kernel
  compromise or undeclared syscall filtering.
- Operator/root-equivalent Docker authority remains a separate later credential and posture.
