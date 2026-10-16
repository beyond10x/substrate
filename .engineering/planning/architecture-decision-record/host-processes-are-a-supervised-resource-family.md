---
format: aep.planning-md/3
id: architecture-decision-record:host-processes-are-a-supervised-resource-family
kind: architecture-decision-record
status: proposed
title: Host processes are a supervised resource family
relations:
- decides: epic:host-process-supervision
revision: 1
---
## Decision
Add a host process (supervised process) resource family, distinct from confined exec and sessions, decided in `adr/0036-host-processes-are-a-supervised-resource-family.md`. A host process is started with argv, cwd, an explicit environment, an owner (session id and name), a reason and a lease, under the explicitly requested `host` profile: the caller's real filesystem and network, no bubblewrap, recorded and reported as `confinement: none`. It runs in one cgroup of its own under the delegated root, so a stop, a lease expiry or a hard deadline (timeout or expires-at) kills the whole tree, and the record names exactly one `ended_by`. It is stopped by id or by owner session, listed with observed CPU, memory and listening ports, logged to a bounded file under daemon state, and its lifecycle is a set of new `process.*` variants in the existing event log, with output lines served only by a per-process stream. Docker and k3d records with restart policy `no` and owner/lease labels are the minimum slice of the workloads and endpoints families, and adoption imports survivors as ownerless records.

## Safety boundary
The `host` profile is never a default and never a fallback from a confinement failure: exec still answers `exec.sandbox-unavailable`. It is served only on the personal Unix socket to a peer whose uid equals the daemon's own, and is refused under the static-bearer TCP listener, hosted admission and network session authority. The subject stays derived from kernel peer credentials; the owner session is a label beneath it.

## Wire and invariants
The family rides a successor bundle after 0.18.0, not a rewrite of any released bundle (invariant 6); every operation is durable before dispatch, deadline and lease-expiry stops included (invariant 5); every new document is classified and validated by `check-json` (invariant 7). Scheduling policy and product policy stay out. The ESS domain lives under `spec/host-processes/`.

## Evidence
On 2026-10-16, 57 processes left behind by ended sessions were found on one host across six mechanisms, 24 of them wait loops polling logs or files.
