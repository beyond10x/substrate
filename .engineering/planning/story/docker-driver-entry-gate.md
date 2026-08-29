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
revision: 2
---
# Story: The Docker driver entry gate is proven before any Docker code

## Outcome

Plan 03's three unchecked entry criteria are proven mechanically, so a second driver tests the
contract instead of the contract bending around it.

## Context

`docs/plan/03-container-driver.md` § *Entry criteria* names four; the first (host conformance)
is met per `STATUS.md`. The others are unchecked claims: driver ports contain no host library
types; capability predicates and applied-enforcement observations are stable; the security design
states which Docker authorities are root-equivalent. `ROADMAP.md` forbids starting phase 5 while
a phase-4 exit criterion is open, so this story depends on the three byte-plane stories and stays
`draft` until they are implemented.

## Acceptance

A structural test fails on the first `substrate-host` type crossing the driver port, the host
driver passes the workspace/exec conformance journey through a driver-parameterised harness, and
design 04 names root-equivalent Docker authorities — landed in the change that moves plan 03 to
*active* and `ROADMAP.md` phase 5 to *in progress*.

Evidence that satisfies it:

- `driver_port_has_no_host_types` in `crates/substrate-daemon` (only `substrate-wire` types cross);
- the conformance harness takes the driver as a parameter and the host driver passes unchanged;
- `docs/design/04-security-and-isolation.md` gains the section: daemon socket, `--privileged`,
  host network/PID namespaces, arbitrary bind mounts — visible deployment facts, never defaults;
- plan 03 status and `ROADMAP.md` phase 5 change in the same commit, and not before phase 4
  reads *complete*.
