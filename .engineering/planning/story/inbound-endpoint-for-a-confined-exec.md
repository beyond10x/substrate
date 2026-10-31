---
format: aep.planning-md/3
id: story:inbound-endpoint-for-a-confined-exec
kind: story
status: draft
title: One declared inbound port is served from a confined exec
summary: 'Glovebox wave 4: an ADR decides how an inbound endpoint coexists with exec.no-egress, then a successor bundle and the host serve one declared port.'
owner: substrate
tags:
- glovebox
- network
revision: 1
---
# Story: one declared inbound port is served from a confined exec

Part of Glovebox (atlas `initiative:glovebox`, `docs/design/glovebox.md`), wave 4.

## Why

- Glovebox requirement 1 names "the inbound port it may serve (port 80 in the example)"
  (`atlas/docs/design/glovebox.md:19-21`), and its gap table records "Inbound port | no: endpoints
  are absent from the wire" (`atlas/docs/design/glovebox.md:70`).
- Substrate states the same: "Workloads, images, volumes, endpoints and the Docker, Kubernetes and
  Firecracker drivers are absent from the wire rather than stubbed" (`STATUS.md:182-183`).
- Design 04 already separates the vocabulary: "egress, listening sockets, and exposed endpoints are
  separate capabilities rather than one boolean" (`docs/design/04-security-and-isolation.md:87`),
  and ADR 0013 repeats it (`adr/0013-egress-apertures-are-declared-by-the-operator.md:16-18`).
- A confined exec runs in "a new network namespace with no usable interface"
  (`docs/design/04-security-and-isolation.md:127`), published as the `exec.no-egress` fact
  (`crates/substrate-wire/src/lib.rs:3579-3580`). Nothing today carries a connection into that
  namespace.
- Atlas leaves the coexistence open: "Inbound endpoints touch Substrate's no-egress floor
  (`exec.no-egress`); the Substrate ADR for them decides how an inbound port coexists with that
  fact" (`atlas/docs/design/glovebox.md:105-106`).

## What changes

1. **An ADR first** (invariant 8, `AGENTS.md` § Invariants). It decides at least:
   - who declares an inbound endpoint: operator configuration, as ADR 0013 does for egress, or the
     start request, and what a request may and may not widen;
   - how a connection reaches the child's network namespace without giving the child an interface
     that can egress. A relay that accepts on the host and hands the child a connected stream,
     mirroring the egress aperture relay, is one candidate (inferred, not decided);
   - how the endpoint coexists with `exec.no-egress`: whether that fact stays `true` for an exec
     that serves an endpoint, and if not, which new fact states the narrower guarantee. A silent
     downgrade of `exec.no-egress` is not allowed (invariant 3);
   - which host address and port the endpoint binds, and who may connect (loopback only, a
     declared address, or a further authority);
   - bounds: connection count, bytes, and lifetime against the exec's lease;
   - the capability fact, the start field, the applied record and each named refusal.
2. After the ADR is accepted, a successor contract bundle cut with `cargo xtask render-bundle`,
   adding the fact, the optional start field, the applied record and the refusals, each held to
   its `substrate_wire` constant (invariant 6).
3. The host driver serves it; a daemon without the mechanism keeps the fact absent and refuses a
   start that names an endpoint before dispatch.

## Acceptance

- The ADR is accepted and indexed in `adr/README.md`, and `cargo xtask check-adrs` passes.
- A successor bundle publishes the endpoint fact, start field, applied record and refusals;
  `cargo xtask check-bundles` and `cargo xtask check-json` pass with it in `scripts/gate.sh`.
- Portable lane (`crates/substrate-daemon/tests/runtime_vectors.rs`): the fact is absent and a
  start naming an endpoint is refused by name before dispatch.
- Delegated lane (`bash scripts/delegated-lane.sh`): a confined exec serves one declared port; a
  client on the host connects, sends a request and reads the child's answer; the same child still
  cannot open an outbound connection to an undeclared destination; the endpoint is gone after the
  exec ends.
- `exec.no-egress` is either unchanged and still true for that exec, or the ADR names the fact
  that replaces it for endpoint-serving execs, and the vectors assert that fact.

## Blocked on / Depends on

- Blocked on the ADR above (invariant 8). No code before it is accepted.
- Depends on nothing outside Substrate. Mantle's `story:manifest-declares-the-box` consumes it and
  refuses the inbound-port field until a Substrate release serves it.
