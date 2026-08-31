---
format: aep.planning-md/1
id: story:remote-clean-room-conformance
kind: story
status: proposed
title: A clean-room client proves remote Substrate
summary: A black-box Rust runner verifies HTTPS/WSS, auth, durable recovery and event semantics against any deployed instance.
owner: substrate
tags:
- conformance
- remote
- testing
relations:
- decomposes: epic:remote-serving
- depends_on: story:remote-sdk-transport
- depends_on: story:node-bound-kubernetes-serving-profile
- depends_on: story:operational-health-and-telemetry
revision: 2
---
# Story: A clean-room client proves remote Substrate

## Outcome

A deployed Substrate endpoint can be certified from outside its process, container and repository checkout using only its public contract and a scoped test authority.

## Acceptance

A Rust conformance runner accepts an HTTPS endpoint, trust roots and a token-provider command or file descriptor without persisting credentials. It verifies machine facts, workspace/file/exec lifecycle, operation-id recovery after a deliberately lost response, restart-to-unknown behavior, event ordering and retention gaps, TLS/auth negatives, raw-pipe or PTY WSS attachment when advertised, exact resource observations and cleanup. Capability-dependent cases are reported passed, failed or absent; absent is never counted as passed. The runner produces a machine-readable signed report with endpoint identity, advertised contract, capability snapshot digest and case inventory but no credential, argv output or tenant data.

## Boundaries

The runner is a client and links no daemon, host driver or store implementation. It does not provision clusters or choose a production fleet.
