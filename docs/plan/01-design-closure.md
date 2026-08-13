# Plan 01: design closure

**Status:** ready for review · **Date:** 2026-08-13

Implementation begins only after this gate is accepted. Closure does not require deciding every
future feature; it requires an explicit v1 answer or an explicit deferral that leaves the minimum
host slice coherent.

## Required decisions

- [ ] Fix v1 resource families and name the operations included in the minimum host slice.
- [ ] Fix the canonical request, response, event, error, capability, and operation-ledger envelopes.
- [ ] Decide one active driver versus multiple active drivers per daemon.
- [ ] Decide durability boundaries before and after driver dispatch.
- [ ] Fix the minimum Linux filesystem, process-tree, environment, sandbox, and network guarantees.
- [ ] Decide personal-mode authentication and token lifecycle.
- [ ] Decide secret-slot delivery for the host driver.
- [ ] Decide lease defaults, restart recovery, event ordering, and retention semantics.
- [ ] Fix PTY/session authority shape or explicitly defer sessions beyond the first host slice.
- [ ] Decide bulk workspace transfer limits.

## Required design artifacts

- [ ] Every open question in Designs 01–06 is answered or assigned to a later named phase.
- [ ] The domain model and wire use one vocabulary from [`glossary.md`](../../glossary.md).
- [ ] The stack integration table is reviewed against connectors Design 03 and umbrella dependency
  rules.
- [ ] Threat cases have an expected allow/refuse/unserved outcome without implementation details.
- [ ] A conformance-test inventory exists for the driver port and public wire.
- [ ] No design requires Flux, sibling checkout paths, or a cloud-only domain rule.

## Exit statement

When complete, this document will record the accepted v1 scope and link the ADRs created from the
decisions. Until then, roadmap phase 2 remains blocked and the repository contains no implementation
workspace.
