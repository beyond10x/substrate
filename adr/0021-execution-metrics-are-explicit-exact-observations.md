---
status: accepted
date: 2026-08-31
---

# ADR 0021: execution metrics are explicit exact observations

## Context

Substrate reads cumulative CPU to enforce a budget and asks the kernel to enforce memory and
process ceilings, but those facts disappear when the cgroup is removed. Clients cannot account for
a completed run, and the existing cancelled state does not distinguish timeout, CPU exhaustion or
memory exhaustion. Automatically adding fields would also change closed responses for every legacy
exec request.

## Decision

Resource measurement is explicitly selected by `measurements: ["resource-usage"]`. A selected exec
reports exact monotonic and kernel counters live and in its durable terminal observation. The
surface includes wall duration, CPU, current/peak memory, current/peak processes, cgroup limit/OOM
events, block I/O, and requested scratch usage. It does not report mean memory or a retained time
series.

GET returns the latest resource observation. A WebSocket provides immediate and one-second
latest-wins samples, never backpressures the run, and emits its terminal frame only after durable
commit. A requested measurement that becomes unavailable ends the run with a named refusal rather
than an estimated value. Timeout, CPU and observed OOM termination are named separately.

## Consequences

Platform clients can audit a run without scraping cgroups or trusting client-side clocks. Legacy
requests keep their old bytes. Consumers that need means, percentiles or long histories calculate
them in an external telemetry system from live observations; Substrate remains the source of exact
execution facts rather than a metrics warehouse.

The complete field and delivery semantics are in
[design 17](../docs/design/17-resource-accounting-and-storage-quotas.md).
