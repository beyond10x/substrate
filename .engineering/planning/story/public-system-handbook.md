---
format: aep.planning-md/1
id: story:public-system-handbook
kind: story
status: implemented
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
revision: 9
---
## Outcome

Apply the completed Connectors documentation treatment to Substrate: a coherent public system explanation, one connected practical example, explicit model and implementation boundaries, readable diagrams and tables, and verified publication.

## Grounding

The public source allowlist is website/docs/**/*.md in b10x.docs.yaml. The original guide set split boundary, confinement, operations and route coverage across separate pages. Public status and contract prose incorrectly described Git materialization as absent, and release summaries still led with 0.5.0. Current Rust types, daemon routes, store transitions and the signed 0.7.3 release establish the implemented 0.16.0 wire surface.

## Scope

- README.md: public entry point, current availability and canonical reader links.
- website/docs: public system/model chapter, connected operation journey, checked examples, route/status corrections and current MCP image identity.
- website/sidebars.ts: include the model chapter in the existing documentation validator.
- CHANGELOG.md: describe the public handbook improvements.
- Website source lock and Atlas-owned snapshot: required delivery inputs. Existing Docs System and Website runtime pins were retained.

## Implementation

The handbook maps the actual wire, daemon, store, Linux host, SDK, MCP and contract-tooling responsibilities. It covers resources, command/operation identity, event vocabulary, capabilities, authority, leases and recovery without implying that future resource families are served.

It states the actual derivation boundary: JSON schema shapes, CLI parsing, handlers and state transitions have authored owners; versioned renderers and conformance checks connect contract artifacts to implementation. There is no whole-system ESS or Entity Runtime derivation. Session resources have typed lifecycles, but their public events are operation-ledger projections rather than a separate session.* vocabulary.

A single workspace/input/exec journey separates start-operation completion from process termination, explains stored-answer replay and deliberate new attempts, and distinguishes reconciliation snapshots from filesystem backups. The exact quickstart blocks use a private runtime directory, checked response envelopes and IDs, explicit file-read mode, and cleanup. The bounded-command guide checks required facts before creation, uses per-run mutation IDs, validates terminal state and stops polling after a fixed number of samples.

Visual review caught overlapping labels on the first subsystem diagram. The final diagram combines the store labels and uses one bidirectional host edge. Browser checks verify that the final rendered labels are distinct.

## Validation evidence

- Exact quickstart Bash blocks passed against the existing daemon: workspace create, file write, bounded read and destruction. Missing-capability preflight and HTTP refusals stop the command script before dependent effects.
- All five command-guide mutation inputs validate against the frozen 0.16.0 JSON schemas. Bash syntax, repository links and whitespace checks pass.
- Substrate PR #89 and diagram correction PR #90 passed the full component gate, documentation build and CodeQL checks. Final component gate: https://github.com/beyond10x/substrate/actions/runs/33976024621.
- Website commit f212042444c80d41a31a982f65cb28a94d3d47d6 passed the local full gate and CI: https://github.com/beyond10x/website/actions/runs/33976767286. The local artifact contains 349 routes and 1305 provenance-listed files; navigation, search, code rendering and the artifact crawl pass.
- Atlas verify-portal passed against the final artifact: 23 locked public sources, 24 surfaces and 50 delivery records.
- The final local artifact and live model/operations pages passed browser checks in actual light and dark modes at 1440, 320, 390 and 720 CSS pixels, with 720 at device scale 2 for 200% reflow. Checks cover intrinsic diagram label size, distinct edge labels, keyboard panning, every table column, semantic headers and absence of whole-page horizontal overflow.

## Publication evidence

The final public source commit is 6baba6888d2d26a2d64287fc3a06c8037c4ed504, merged through PR #90 after all checks passed. Its immutable documentation source bundle run 33976426529 succeeded. Both PR merge commits were verified GitHub merges with b10x-bot as author and web-flow as committer; no attestation-only commit was added.

The published Website lock and snapshot commit is f212042444c80d41a31a982f65cb28a94d3d47d6. Atlas publication run 33976767478 succeeded using current control commit 974b2a2bc4896bd76293a734f36ac254895221c4. Root deployment https://github.com/beyond10x/beyond10x.github.io/actions/runs/33976983421 succeeded.

Public PROVENANCE.json confirms Substrate source commit 6baba6888d2d26a2d64287fc3a06c8037c4ed504 and source-set SHA-256 4ecd2dcc8c884ca303b8b24a5db79b179bb90e4c3dae8200ac62e52caa5b6691. The existing runtime remains 815fad1b977992d01695f6b5c79495c02576212b; no runtime or facade migration was needed.

Canonical entry: https://beyond10x.github.io/docs/substrate/concepts/model/.
Atlas verify-pages passed after publication: 36 repository states, 25 Pages repositories and 50 delivery routes agree on one Website commit.

## Limits and execution boundary

Successful delegated execution is not claimed by the portable smoke checks: the local host did not advertise the full confinement/quota facts, and the example correctly stopped before creation. No application code or frozen wire contract was changed.

The primary-workspace docs reconcile check remains independently refused by its outdated collector on agentide's v4 manifest. The exact Website collector, artifact gate and live delivery checks passed; this story did not modify unrelated sibling repositories to repair primary-workspace drift. Existing planning warnings for older unscoped stories and prose-only review records were preserved.

This was one interactive documentation story under the operator's request, with no parallel agent panel or broad local compiler rebuild.