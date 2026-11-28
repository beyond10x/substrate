---
format: aep.planning-md/3
id: story:local-endpoint-attributed-to-its-run
kind: story
status: draft
title: A run reaches a local endpoint whose connections the node agent attributes to that run
summary: 'Consumer gap 3: an aperture relayed to a per-run Unix socket the node agent serves; design change within ADR 0013.'
owner: substrate
relations:
- decomposes: epic:long-lived-service-on-the-host-driver
revision: 2
---
# Story: a run reaches a local endpoint whose connections the node agent attributes to that run

## Where the requirement comes from

Consumer gap 3 of the `weft` repository (`cloudspec` `docs/platform-ports.md` § 10, commit
`2ed72c2`): "A per-instance local endpoint (N4): a socket the node agent serves inside each exec's
network namespace, from which the agent learns which instance called." The stakeholder is the
session that maintains `weft`.

## Why

- An egress aperture (ADR 0013) is the one way a confined run reaches anything outside its
  namespace: a listener on `127.0.0.n` inside the namespace, relayed to one operator-declared TCP
  destination. Every run that names it reaches the same destination, so the destination cannot
  tell runs apart.

## What changes

Decided here; a design change within ADR 0013, recorded by an amendment or a new ADR before code:

1. A **local endpoint** is an operator-declared aperture whose destination is a directory of Unix
   sockets the node agent serves, not a TCP tuple. A run names it like any aperture.
2. For each run that names it, the daemon relays connections from the in-namespace listener to the
   socket `<directory>/<run id>` and to no other. The agent attributes a connection by the socket
   it arrived on. The run sees only its listener address; it never names a socket path.
3. If that socket is absent at connect time the connection is closed; the run gets no fallback.
4. Applied record: the aperture's name, listen address and the run id the socket is keyed by; no
   host path.

## Acceptance

A weft test (`weft` story `ports-node-endpoints-local`) can check:

- Two runs naming the same local endpoint each connect to their listener; the agent's socket for
  run A receives only A's connection and the socket for run B only B's.
- A run that names no local endpoint has no listener for it.
- With the agent's socket for a run absent, that run's connection is closed and no other run's
  socket receives it.
- The applied record names the endpoint and listen address and carries no host path.
- Portable lane: without the mechanism the fact is absent and a start naming a local endpoint is
  refused by name before dispatch.

## Blocked on / Depends on

- Nothing outside ADR 0013's mechanism. Workloads get it with `story:host-driver-supervised-workload`.
