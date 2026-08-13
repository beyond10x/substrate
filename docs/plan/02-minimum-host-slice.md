# Plan 02: minimum host slice

**Status:** planned after design closure · **Date:** 2026-08-13

The first implementation proves the contract on one Linux host without Docker, Kubernetes, cloud,
connectors, Flux, or autodev in the build graph.

## Slice

1. Probe and report machine facts required by the slice.
2. Create an empty confined workspace beneath a configured root.
3. Read, list, atomically write, and delete bounded workspace files.
4. Start one argv-only exec with cleared/shaped environment, timeout, output cap, and a required
   workspace sandbox.
5. Observe exec state and terminal exit without treating a non-zero program exit as a wire error.
6. Signal/cancel an exec and clean up its process tree.
7. Persist operation ids sufficiently to reconcile a lost answer.
8. Destroy the workspace and report observed absence.

## Acceptance evidence

- A black-box client completes the journey using only the versioned wire.
- Lexical and symlink escapes, unavailable sandbox, excess output, invalid operation replay, and
  daemon credential inheritance have negative tests.
- Responses distinguish request, acceptance, applied enforcement, and observed result.
- Killing the client after dispatch can be reconciled with the original operation id.
- The repository builds and tests without a Flux checkout or any consumer source.
- Machine facts never claim a capability that the running host failed to probe.

## Explicitly later

Git clone/snapshot transport, leases, PTY sessions, workloads, images, volumes, endpoints, Docker,
Kubernetes, connector projection, hosted identity, and fleet placement do not enter the first slice
unless design closure proves one is necessary for correctness.
