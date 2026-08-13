# Plan 01: design closure

**Status:** accepted · **Date:** 2026-08-13

This gate was accepted after every founding question received a v1 answer or a named deferral that
leaves the minimum host slice coherent. It closes architecture; implementation evidence remains in
the exit criteria of the phase that owns it.

## Required decisions

- [x] Fix v1 resource families and name the operations included in the minimum host slice.
- [x] Fix the canonical request, response, event, error, capability, and operation-ledger envelopes.
- [x] Define the canonical machine-readable specification bundle, version/hash manifest, and
  conformance fixtures; do not call Markdown prose the released wire artifact.
- [x] Define the connectors projection manifest and total field mapping. Phase 6 owns the generated
  provider's byte-for-byte implementation proof; no schema-identity claim is permitted.
- [x] Decide one active driver versus multiple active drivers per daemon.
- [x] Decide durability boundaries before and after driver dispatch.
- [x] Fix the minimum Linux filesystem, process-tree, environment, sandbox, and network guarantees.
- [x] Fix authenticated personal mode, one-daemon/one-trust-domain tenancy, subject-scoped resource
  ownership, and token lifecycle.
- [x] Align every resource family with an explicit local scope and define non-inheriting `admin`.
- [x] Decide secret-slot delivery for the host driver.
- [x] Decide lease defaults, restart recovery, event ordering, and retention semantics.
- [x] Fix PTY/session authority shape or explicitly defer sessions beyond the first host slice.
- [x] Decide bulk workspace transfer limits.
- [x] Fix named Git source/remote credential binding, destination aperture, DNS rebinding,
  redirects, proxies, submodules, LFS, helpers, hooks, and immutable-ref behavior.
- [x] Decide capability-snapshot invalidation and dispatch-time security rechecks.

## Required design artifacts

- [x] Every open question in Designs 01–06 is answered or assigned to a later named phase.
- [x] The domain model and wire use one vocabulary from [`glossary.md`](../../glossary.md).
- [x] The stack integration table is reviewed against connectors Design 03 and umbrella dependency
  rules.
- [x] Threat cases have an expected allow/refuse/unserved outcome without implementation details.
- [x] A conformance-test inventory exists for the driver port and public wire.
- [x] Architecture RFCs 0001–0003 and 0005 are accepted or their dependent substrate features are
  explicitly deferred beyond the implementing phase.
- [x] No design requires Flux, sibling checkout paths, or a cloud-only domain rule.

## Exit statement

The accepted v1 scope is recorded by [Design 07](../design/07-specification-and-conformance.md) and
[ADRs 0003–0006](../../adr/README.md). Roadmap phase 2 is unblocked. Its first implementation
deliverables are the development contract bundle, host-driver/wire conformance harnesses, and then
the vertical slice; no later-phase capability may enter merely because implementation has begun.
