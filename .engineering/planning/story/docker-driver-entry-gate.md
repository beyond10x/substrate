---
format: aep.planning-md/1
id: story:docker-driver-entry-gate
kind: story
status: draft
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
revision: 3
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
