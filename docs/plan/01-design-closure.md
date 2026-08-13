# Plan 01: design closure

**Status:** ready for review · **Date:** 2026-08-13

Implementation begins only after this gate is accepted. Closure does not require deciding every
future feature; it requires an explicit v1 answer or an explicit deferral that leaves the minimum
host slice coherent.

## Required decisions

- [ ] Fix v1 resource families and name the operations included in the minimum host slice.
- [ ] Fix the canonical request, response, event, error, capability, and operation-ledger envelopes.
- [ ] Define the canonical machine-readable specification bundle, version/hash manifest, and
  conformance fixtures; do not call Markdown prose the released wire artifact.
- [ ] Define the connectors projection manifest and total field mapping; prove the generated
  provider document byte-for-byte without claiming schema identity.
- [ ] Decide one active driver versus multiple active drivers per daemon.
- [ ] Decide durability boundaries before and after driver dispatch.
- [ ] Fix the minimum Linux filesystem, process-tree, environment, sandbox, and network guarantees.
- [ ] Fix authenticated personal mode, one-daemon/one-trust-domain tenancy, subject-scoped resource
  ownership, and token lifecycle.
- [ ] Decide secret-slot delivery for the host driver.
- [ ] Decide lease defaults, restart recovery, event ordering, and retention semantics.
- [ ] Fix PTY/session authority shape or explicitly defer sessions beyond the first host slice.
- [ ] Decide bulk workspace transfer limits.
- [ ] Fix named Git source/remote credential binding, destination aperture, DNS rebinding,
  redirects, proxies, submodules, LFS, helpers, hooks, and immutable-ref behavior.
- [ ] Decide capability-snapshot invalidation and dispatch-time security rechecks.

## Required design artifacts

- [ ] Every open question in Designs 01–06 is answered or assigned to a later named phase.
- [ ] The domain model and wire use one vocabulary from [`glossary.md`](../../glossary.md).
- [ ] The stack integration table is reviewed against connectors Design 03 and umbrella dependency
  rules.
- [ ] Threat cases have an expected allow/refuse/unserved outcome without implementation details.
- [ ] A conformance-test inventory exists for the driver port and public wire.
- [ ] Architecture RFCs 0001–0003 and 0005 are accepted or their dependent substrate features are
  explicitly deferred beyond the implementing phase.
- [ ] No design requires Flux, sibling checkout paths, or a cloud-only domain rule.

## Exit statement

When complete, this document will record the accepted v1 scope and link the ADRs created from the
decisions. Until then, roadmap phase 2 remains blocked and the repository contains no implementation
workspace.
