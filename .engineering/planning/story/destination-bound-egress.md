---
format: aep.planning-md/1
id: story:destination-bound-egress
kind: story
status: draft
title: Destination-bound egress apertures are declared, verified and refused by name
summary: 'Design 04 section 6: ordinary execution has no egress; an aperture is operator authority matched to a destination. With sealed secret slots this is what unlocks a confined vendor harness.'
owner: substrate
tags:
- daemon
- host
- wire
relations:
- decomposes: epic:byte-plane-completion
- depends_on: story:sealed-secret-slots
revision: 4
---
# Story: Destination-bound egress apertures are declared, verified and refused by name

## Outcome

A confined run reaches exactly the network destinations an operator declared for it — and, with
`story:sealed-secret-slots`, a vendor harness (`codex`, `claude`) can run under substrate
confinement with its model credential and its model endpoint, which design 05 § *Progress* says is
refused until both capabilities exist.

## Context

`docs/design/04-security-and-isolation.md` § 6: egress, listening sockets and exposed endpoints
are separate capabilities; ordinary execution defaults to **no egress**; "an aperture is
deployment/operator authority" matched to a destination. The delegated lane today proves
namespace no-egress and nothing else. Atlas O3/O4 want vendor harnesses driven under the same
governor as the native loop; `autodev` dispatches only `codex` and `claude`
(`atlas/ROADMAP.md`, arrow *autodev → the fleet*). A vendor harness that cannot reach its model
is not confined, it is dead — so secrets and egress are one capability pair, and the review that
filed this story found the backlog held only the first half.

## Acceptance

A run started with a declared egress aperture reaches the declared destination and nothing else,
the applied aperture is an observation in the run's record, and a request for an aperture the
deployment did not declare is `unserved` with the aperture named — proven by the delegated lane
with a real listener inside and outside the aperture.

Evidence that satisfies it, in order:

1. **Before code**: an ADR fixing the aperture shape (host:port or name:port, protocol, whether
   DNS resolution is inside the aperture), the operator declaration surface, and the refusal.
2. A successor bundle adds the request field, the capability fact `network.egress.apertures`, the
   observation, and the refusal; earlier bundle bytes unchanged.
3. Failing-first tests: `egress_defaults_to_none`, `declared_aperture_is_reachable`,
   `undeclared_destination_is_unreachable_and_named`,
   `aperture_outside_operator_declaration_is_unserved`, `applied_aperture_is_observed`.
4. The delegated lane runs the model-free fake app-server against a loopback endpoint inside the
   aperture and proves a second endpoint outside it is unreachable.

## Out of Scope

Public ingress, DNS, TLS termination and tunnels (design 05 § 4). Brokered connector artifacts.

## Open Questions

Enforcement mechanism inside the existing namespace no-egress posture: a veth pair plus nftables
in the sandbox namespace, or a userspace proxy at a declared descriptor. Decides: the ADR. Default
if nobody answers: **namespace + nftables**, because it keeps bytes out of the daemon.

## Design draft — 2026-08-30

`docs/design/10-destination-bound-egress.md` (proposed). Finding that overturns this story's
default: option (a), namespace + nftables, needs root, and the daemon withholds `exec` when it
runs as root (`crates/substrate-host/src/probe.rs:49-51`); bubblewrap 0.11.2 has no `--netns`, so
a prepared namespace cannot be handed in. The draft recommends (c), a per-run forwarder inside the
sandbox netns, owing a `setns` spike on a delegated host before the ADR. `NetworkMode::Aperture`
already exists and is `unserved` twice (`substrate-wire/src/lib.rs:595-599`,
`operations.rs:290-300`, `process.rs:810-815`); the only executed no-egress proof today is one
`connect` to `1.1.1.1:53` (`crates/substrate-daemon/tests/runtime_vectors.rs:620-640`, ported from the retired
`scripts/check-runtime-vectors.py:348-367` on 2026-08-30).
