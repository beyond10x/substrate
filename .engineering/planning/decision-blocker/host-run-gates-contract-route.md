---
format: aep.planning-md/3
id: decision-blocker:host-run-gates-contract-route
kind: decision-blocker
status: open
title: Contract and specification route for host-run gates
relations:
- blocks: epic:host-run-gates
revision: 2
---
# Decision blocker: contract and specification route for the host-run-gates epic

## Questions

1. **Exec usage without `io`** (`story:exec-usage-without-io-controller`). Serving
   `exec.resource-usage` without block I/O changes the wire: `block_io` is `const: true` and the
   two I/O counters are required in every bundle through `0.17.0`.
   - A: cut successor bundle `0.18.0` (`block_io` boolean; `io_read_bytes`/`io_write_bytes` absent
     when it is `false`) with an ADR first. Clients that read the record tolerate absent I/O
     counters when they move to it.
   - B: no change here; the host delegates `io` to the user manager.
   - C: defer.
2. **Writable Git common directory** (`story:linked-worktree-git-common-dir`). A read-only common
   directory needs no contract change (ADR 0010 roots). A writable host bind outside `/workspace`
   is a change to the safety envelope and to ADR 0010.
   - A: read-only only.
   - B: add a writable declared bind (new ADR, successor bundle field).
3. **Specification.** `spec/coverage.md` names cgroup, filesystem and aperture enforcement as
   outside the ESS projection.
   - A: prove these stories with native host tests and the delegated lane, as before.
   - B: extend the ESS specification with a capability and confinement domain first.

## Decision

- Q2: A, read-only only. A writable bind of a host directory outside `/workspace` is not built.
- Q3: A, native host tests and the delegated lane; `spec/coverage.md` keeps confinement outside the ESS projection.
- Q1: open. `story:exec-usage-without-io-controller` stays draft; no successor bundle and no ADR until it is decided.
