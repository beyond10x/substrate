# Plan 04: stack adoption

**Status:** deferred until the public contract is proven · **Date:** 2026-08-13

Adoption proves release independence. Each consumer integrates over the public wire or its own
adapter and remains buildable without a sibling substrate checkout.

## Proof sequence

1. **connectors projection:** mechanically declare the stable substrate operation subset, risk,
   effects, idempotency, capabilities, and events. Govern one operation under a connector grant.
2. **Flux adapter:** map guarded workspace/exec behavior and unanswered outcomes onto Flux-owned
   delegate concepts. The adapter lives in Flux; substrate remains unchanged and Flux-free.
3. **autodev adapter:** implement the existing `Executor` port using pinned workspace materialization,
   bounded exec, lease/reconciliation, and evidence handoff. Scheduling remains in autodev.
4. **agent journey:** demonstrate that the generic agent layer can request execution through an
   agent-owned tool/runner port without importing substrate domain types into its core lifecycle.
5. **hosted composition:** register and select a deployment using identity/cloud trust while serving
   the identical substrate API.

## Compatibility evidence

- Each consumer pins a released protocol/version and runs conformance fixtures.
- No consumer requires a path dependency or unpublished local source.
- Substrate CI does not fetch or build any consumer.
- Credential, principal, grant, placement, and billing semantics remain with their owning layers.
- PTY/tunnel bytes flow directly after governed establishment rather than through connector invoke.

External connector runtime artifacts are not part of this plan. They require the separate
connectors security and supply-chain decision described in connectors Design 03.
