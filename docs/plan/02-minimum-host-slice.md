# Plan 02: minimum host slice

**Status:** active — contract execution readiness · **Date:** 2026-08-13

The first implementation proves the contract on one Linux host without Docker, Kubernetes, cloud,
connectors, Flux, or autodev in the build graph.

The canonical endpoint/envelope set is fixed in
[Design 07](../design/07-specification-and-conformance.md); the Linux enforcement floor is fixed in
[Design 04](../design/04-security-and-isolation.md). Implementation starts with the `0.1.0`
development bundle and failing black-box vectors, not server-only types that later become a spec.

## Active goal: contract execution readiness

Runtime scaffolding remains deliberately closed until this tranche passes. Complete it in this
order:

1. Add one machine-readable operation registry covering exactly the twelve phase-2 method/path
   pairs. Every entry names its closed input/result schemas, risk, idempotency, effects, exposure,
   and required capability predicates.
2. Replace generic `input`, `result`, and `outcome` authority with closed route-specific schemas.
   Represent operation-state invariants and requested-versus-applied confinement structurally.
3. Add exact canonical-hash fixtures: normalized address, RFC 8785 input bytes, length-delimited
   tuple bytes, expected SHA-256, stable replay, and different-input conflict. Prove that operation
   id, request id, headers, bearer material, subject, and deployment are outside the request hash;
   subject and deployment remain part of the ledger key.
4. Convert every prose-backed vector case into a machine-executable fixture with separated trusted
   harness context, setup, wire/driver action, exact expected instance or digest, and observable
   postconditions. Fixture identity must never be accepted from request data.
5. Complete the phase-2 route/error/recovery inventory and every Design 04 threat row, including
   absolute, dangling-link, magic-link, and mount escapes; bounded read/list/write/delete; atomic
   replacement; unauthenticated reachable startup; crash-before/after dispatch; lost-answer
   reconciliation; resource and operation subject isolation; timeout; and post-action observation.
6. Make the dependency-free bundle gate validate schema instances, exact inventory, hash/length/
   media-type coverage, duplicate keys, safe paths, and deterministic source form. Define the
   producer/clean-room runner interface without adding runtime implementation.

The tranche exits when a clean checkout proves the complete bundle is internally executable by a
future producer and independent consumer, with no Flux or sibling checkout. Passing this gate opens
the Rust workspace and minimum vertical slice; it does not claim host-driver conformance before a
driver exists.

## Slice

1. Probe and report machine facts required by the slice.
2. Create an empty confined workspace beneath a configured root.
3. Read, list, atomically write, and delete bounded workspace files.
4. Start one argv-only exec with cleared/shaped environment, timeout, output cap, and a required
   workspace sandbox with no egress.
5. Observe exec state and terminal exit without treating a non-zero program exit as a wire error.
6. Signal/cancel an exec and clean up its process tree.
7. Persist operation ids sufficiently to reconcile a lost answer.
8. Destroy the workspace and report observed absence.

## Acceptance evidence

- A black-box client completes the journey using only the versioned wire.
- Lexical and symlink escapes, unavailable sandbox, excess output, invalid operation replay,
  cross-subject resource and operation-id access, unauthenticated loopback, daemon credential
  inheritance, and stale capability snapshots have negative tests.
- Responses distinguish request, acceptance, applied enforcement, and observed result.
- Killing the client after dispatch can be reconciled with the original operation id.
- The repository builds and tests without a Flux checkout or any consumer source.
- Machine facts never claim a capability that the running host failed to probe.

## Explicitly later

Git clone/snapshot transport, leases, PTY sessions, workloads, images, volumes, endpoints, Docker,
Kubernetes, connector projection, hosted identity, and fleet placement do not enter the first slice
unless design closure proves one is necessary for correctness.
