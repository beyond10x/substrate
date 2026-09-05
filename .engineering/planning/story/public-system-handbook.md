---
format: aep.planning-md/1
id: story:public-system-handbook
kind: story
status: active
title: The public handbook explains Substrate as a system and follows one bounded execution
owner: substrate
tags:
- docs
- o1
- website
relations:
- derived_from: epic:resource-bounded-execution
- informed_by: story:public-practical-execution-docs
scope:
- confidence: cited
  path: CHANGELOG.md
- confidence: cited
  path: README.md
- confidence: cited
  path: website/docs
- confidence: cited
  path: website/sidebars.ts
revision: 6
---
## Outcome

Apply the completed Connectors documentation treatment to Substrate, as requested by the operator: a coherent public system explanation, a connected practical example, explicit model and implementation boundaries, readable diagrams and tables, and verified publication.

## Grounding

The public allowlist is website/docs/**/*.md in b10x.docs.yaml. The current guide set covers boundary, confinement, operations, and routes separately. website/docs/reference/contract.md and website/docs/status.md still say Git workspace materialization is absent despite the implemented 0.16.0 contract and Git source handlers. README.md and the public status page still lead with release 0.5.0; the GitHub latest-release API currently reports 0.7.3. architecture/domain-model.md names wider resource families than the host implements. crates/substrate-wire/src/lib.rs, crates/substrate-daemon/src/app/operations.rs, the store, host adapter and contract checks provide the implementation evidence.

## Scope

- README.md: cited public entry point and stale release summary.
- website/docs: cited public source root; improve the existing chapters and add a public architecture/model coverage page only where the existing pages cannot carry it clearly.
- CHANGELOG.md: cited repository convention for user-visible documentation improvements.
- Website source lock and Atlas-generated snapshot: required delivery inputs after source publication; retain the existing Docs System and Website runtime pins.

## Approach

Use one illustrative workspace, operation, exec and event sequence to explain ownership, durable admission, confinement, observations, retries, leases and recovery. Map resource families, commands, events, capability facts, and derivation to their actual Rust and contract sources. Distinguish shipped host behavior, conditional capabilities, declared future families, and generated output. Preserve existing public URLs and keep internal plans, ADRs, reviews and work logs outside the public allowlist. Reuse the shared diagram/table rendering shipped for Connectors.

## Acceptance

Public claims agree with current implementation and release evidence. Runnable examples validate response envelopes and terminate on refusals rather than continuing with null ids or polling forever. Diagram labels and table columns remain readable on mobile, desktop, both themes and zoom. The Substrate gate and relevant Website/Atlas artifact and live-delivery checks pass. No application, frozen wire contract or shared rendering behavior changes.

## Execution boundary

This is one interactive documentation story under the operator's instruction to do the same completed work for Substrate. No decomposition or parallel agent panel is needed for a single story. Validation stays scoped to Substrate and required publication checks, with disposable outputs reviewed and cleaned to avoid the previous disk pressure.

## Implementation evidence

The public system/model chapter maps actual crates, typed resources, commands, event coverage and authored/generated boundaries. Session lifecycle events are documented as operation-ledger projections. Status and contract pages now agree on Git materialization and release 0.7.3 / wire 0.16.0; historical contributor claims in README and the MCP image example were corrected too. The existing sidebars.ts validator includes the new chapter.

The exact quickstart Bash blocks passed against the existing release daemon: create workspace, write input, bounded read and destruction. The full command guide stops before workspace creation when this host lacks required confinement/quota facts. Its helper stops on an HTTP refusal. Bash syntax and repository links pass. No full local Rust rebuild is needed; PR CI will run the component and documentation gates. Successful delegated execution is not claimed by these portable smoke checks.

Atlas docs reconcile --check against the primary workspace currently refuses on agentide's v4 manifest with the primary workspace collector. That unrelated workspace/tooling mismatch is recorded, not repaired by this docs story. Publication will use Website's exact pinned collector and Atlas's artifact/live delivery gates.

Source publication, rendered accessibility checks and live provenance verification remain pending.
