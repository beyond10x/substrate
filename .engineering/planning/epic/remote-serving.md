---
format: aep.planning-md/3
id: epic:remote-serving
kind: epic
status: active
title: Remote serving and hosted trust
summary: Production HTTPS/WSS, scoped Identity admission, remote SDK transport and black-box conformance make Substrate safely addressable by agent-platform and other services.
owner: substrate
tags:
- devcenter
- remote
- security
relations:
- depends_on: epic:release-hardening
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-08-31T22:54:30Z", actor: "human:timo", revision: 2, imported: true}
- {from: "proposed", to: "active", at: "2026-09-01T05:57:32Z", actor: "human:timo", revision: 3, imported: true}
---
# Epic: Remote serving and hosted trust

## Outcome

Agent-platform, Devcenter-facing composition, and other independently deployed services can address a Substrate instance over a production HTTPS/WSS endpoint without weakening the local Unix-socket trust model.

## Boundaries

Substrate authenticates and authorizes execution-data-plane operations; it does not store product sessions, choose tenant policy, schedule a fleet, or embed Identity. The Unix socket remains supported. Static-bearer plaintext TCP remains development-only and cannot be enabled as a hosted posture.

## Delivery

The track promotes an implemented contract frontier, adds a rustls control listener, verifies the accepted hosted trust envelope, extends the Rust SDK with remote transport, publishes operational probes and metrics, and proves the service from a clean-room client. Every wire change cuts a successor bundle; no frozen bundle is changed.
