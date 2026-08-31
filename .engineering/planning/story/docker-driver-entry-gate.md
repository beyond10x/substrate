---
format: aep.planning-md/1
id: story:docker-driver-entry-gate
kind: story
status: proposed
title: The Docker driver entry gate is proven before any Docker code
summary: Plan 03's three unchecked entry criteria, proven mechanically; held until phase 4 exits by roadmap order.
owner: substrate
tags:
- daemon
- docs
- host
relations:
- decomposes: epic:container-driver-entry
- depends_on: story:pty-sessions
- depends_on: story:sealed-secret-slots
- depends_on: story:network-session-authority
revision: 5
---
# Story: The Docker driver entry gate is proven before any Docker code

## Outcome

Plan 03's remaining entry criteria are proven mechanically, so a second driver tests the contract
instead of the contract bending around it. Plan 03 moves from *deferred* to *active* and
`ROADMAP.md` phase 5 to *in progress* in the same change.

## Context

`docs/plan/03-container-driver.md` § *Entry criteria* names four. The first (host conformance) is
met per `STATUS.md`; the second is `story:driver-port-carries-no-host-types`, split out on
2026-08-29 because it does not wait for phase 4. This story keeps the two that do: a
driver-parameterised conformance journey, and the security-design section naming root-equivalent
Docker authorities — both of which shape the Docker driver itself, which `ROADMAP.md` holds
behind phase 4's exit.

## Acceptance

The host driver passes the workspace/exec conformance journey through a harness that takes the
driver as a parameter, design 04 names root-equivalent Docker authorities, and plan 03 and
`ROADMAP.md` phase 5 change together — not before phase 4 reads *complete*.

Evidence that satisfies it:

- the conformance harness takes the driver as a parameter and the host driver passes unchanged;
- `docs/design/04-security-and-isolation.md` gains the section: daemon socket, `--privileged`,
  host network/PID namespaces, arbitrary bind mounts — visible deployment facts, never defaults;
- plan 03 status and `ROADMAP.md` phase 5 change in the same commit.

## Design draft — 2026-08-30

`docs/design/15-docker-driver-entry-gate.md`, **proposed**. Claims no ADR number.

| decision | chosen | alternative's cost |
|---|---|---|
| socket acquisition | an explicit flag only; no autodetect, no `DOCKER_HOST` | discovery turns a `chmod` elsewhere on the host into a silent capability here |
| root-equivalence | published as the fact `exec.host-equivalent-authority` | a driver-name check makes the driver part of the contract (invariant 4) |
| refused options | the driver builds a closed spec and the probe must pass **with the option applied** | a denylist grows by accident and is never the enforcement |
| sealed slots | **absent** on containers | there is no `pre_exec` — the runtime forks the child, so passing the value through the runtime API is the leak ADR 0012 closes |
| apertures | **absent** until a spike observes the `setns` handback | asserting it repeats what design 10 refused |
| new refusal codes | one, `exec.container-option-in-request` | more would duplicate `exec.sandbox-unavailable`, which reads facts and never a driver kind |

Bundle `0.9.0`, provisional — designs 13, 14 and 16 name it too.

Two findings from the draft: the story's acceptance maps plan 03 criterion 3 onto a conformance
harness, but that is criterion 1; and `crates/substrate-daemon/tests/driver_port.rs:7` cites
`substrate-host/src/lib.rs:171` for `pub trait Driver`, which is `:201` at `d65db79` — a
text-scanning test, so nothing is broken, but the citation is stale.
