---
format: aep.planning-md/3
id: approval-record:adr-0109-confinement-plan
kind: approval-record
status: active
title: ADR 0109 confinement plan accepted for implementation
tags:
- adr-0109
- non-interactive
relations:
- decides: story:ess-pin-0-70-0
- decides: story:confinement-domain-specification
- decides: story:confinement-contract-on-linux-and-local
- decides: story:macos-adapter-on-seatbelt
- decides: story:macos-seatbelt-policy-runs-cargo-test
- decides: story:systemd-scope-bounds-provider
- decides: story:oci-adapter
- decides: story:pod-adapter
- decides: story:confinement-consumer-surface
- decides: story:sandbox-repository-parity
revision: 2
transitions:
- {from: "draft", to: "active", at: "2026-10-27T01:14:24Z", actor: "human:timo", revision: 2}
---
# ADR 0109 confinement plan: accepted for implementation

## Decision

Atlas ADR 0109, "Confinement is one Substrate contract with an adapter per mechanism", accepted
2026-10-27, on atlas `main` at `4a2bcf13`
(`architecture/adr/0109-confinement-is-one-substrate-contract-with-adapters.md`). Decision 8: the
contract families process run, process watch, output read, wait, workspace, filesystem view,
network ingress and egress, limits and audit; each adapter declares its subset as capability facts,
and a missing family is refused by name. Decision 9: F1 B, F2 A, F3 A.

## The operator's words

The operator, 2026-10-27, relayed verbatim by session `b10x-chef-orchestrator`:

> this makes sense. I'm accepting the architectural decision.

> substrate should come with various contracts, like for example run a process, watch a process,
> get the output, wait for some, whatever. And then the various adapters which we were choosing,
> they support some of the contracts; not everyone would necessarily support each of them.

The direction it records, the same day: "maybe for simplicity, all the sandboxing code, all the
adapters and stuff, should live actually in substrate. So substrate is just the ground the agent is
standing on. It would provide workspaces, file system access, networking, and confinement of such.
Also auditing and limiting, rate limiting, I/O and so on."

## The grant this run acts under

`b10x-chef-orchestrator`, 2026-10-27, naming this item under the class-C grant of the collection's
session instructions: "Approved under the grant ...: planning-store writes, waves, bot PRs, merges,
releases through substrate's own process." Its order: the ESS story first, S0 second, one story per
adapter, the consumer surface, the sandbox-repository parity story; waves of at most four units,
each released through substrate's own process.

## Delivery, as the operator directed in this session (2026-10-27)

> use /aep:implementing - do all changes against a single integration branch, provide 1 single PR
> in the end

> use sub-agents for impl. - in general use sub-agents, do not block the main session

This replaces the per-wave release of the coordinator's message: units are implemented by
sub-agents in waves of at most four, each unit merges into one integration branch, and the epic
ships as one pull request and one release through substrate's own process.

## What this record decides

The stories of `epic:confinement-contract-family`, as revised after the plan-critic panel, move
`draft -> proposed -> active`. Non-interactive run (a sub-agent dispatch): no operator turn; the
coordinator's message above is the approval, and this record replaces the question it would have
been asked ("may these stories be accepted for implementation?").

## Calls taken in the operator's absence

- `local` serves no family that would claim confinement (filesystem view, network, limits); a
  profile naming one under `local` is refused by family name rather than run unenforced
  (invariant 3). `systemd` wrapping `local` is how a bounded unconfined run is had.
- macOS is two stories (S1 adapter, S2 policy for `cargo test`), as ADR 0109's consequences list
  them, not one.
- Every adapter's scenarios are written in the ESS story, so later adapter stories add a test file
  each and never touch the committed suite.
- The ESS pin moves to the newest release (0.70.0) in its own story, `story:ess-pin-0-70-0`,
  before the domain story (critic round 1 split it out).
- `story:pod-adapter` depends on `story:oci-adapter` so their edits to
  `.github/workflows/gate.yml` land in order.
