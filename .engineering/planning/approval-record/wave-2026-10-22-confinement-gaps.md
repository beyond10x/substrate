---
format: aep.planning-md/3
id: approval-record:wave-2026-10-22-confinement-gaps
kind: approval-record
status: active
title: 'Wave 2026-10-22: working directory, several apertures, read-only roots by descriptor'
relations:
- decides: story:exec-working-directory
- decides: story:several-apertures-per-run
- decides: story:read-only-roots-bound-by-descriptor
revision: 3
transitions:
- {from: "draft", to: "active", at: "2026-10-22T04:37:18Z", actor: "agent:b10x-chef-orchestrator", revision: 3, executor: "agent:substrate-wave-20261022"}
---
# Wave 2026-10-22: working directory, several apertures, read-only roots by descriptor

Integration branch `wave/2026-10-22-confinement-gaps`, base `216cfd07` (main, release 0.8.0). One
full gate and the delegated lane on the final head, one pull request, then release 0.9.0 for Loom
0.34.0.

## Approval

b10x-chef-orchestrator, 2026-10-22, under the grant in the collection's session instructions: "file
and implement, as one wave, the confinement gaps Loom and takt hit on substrate 0.7.18 ... First
check what 0.8.0 (workspace.adopt) already covers and drop those. Additive wire changes only; if any
item needs a new substrate ADR or a breaking wire change, stop and report it with options. Release
when the wave merges and tell me the tag".

Finding: 0.8.0 covers none of the four. Item 1 (writable mount outside `/workspace`) needs an ADR.
Orchestrator, 2026-10-22: "Agreed: ADR is Timo's; I asked him A/B/C. Ship 2-4 as the release now and
send me the tag." Then: "Accepted: draft B (proposed, not accepted, no implementation)". Item 1 is
`story:writable-host-roots`, blocked by
`decision-blocker:writable-host-roots-adr`.

## Units

| unit | story | lands on |
|---|---|---|
| u1 | `story:read-only-roots-bound-by-descriptor` | `crates/substrate-host/src/process.rs` (roots admission, argv) |
| u2 | `story:several-apertures-per-run` host mechanism | `crates/substrate-host/src/egress.rs`, `process.rs` install path |
| u3 | `story:exec-working-directory`, `apertures` wire, bundle `0.21.0`, daemon, vectors | `crates/substrate-wire/`, `xtask/bundle-source/0.21.0/`, `contracts/substrate-wire/0.21.0/`, `scripts/gate.sh`, daemon, `runtime_vectors.rs` |

u1 and u2 run in parallel; u3 starts after u2 merges (it wires `apertures` into the u2 mechanism).
Adversary on u1 and u2 (confinement), security review on u3.
