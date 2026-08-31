---
format: aep.planning-md/1
id: story:hosted-trust-envelope-admission
kind: story
status: proposed
title: Hosted requests use the accepted trust envelope
summary: Identity-issued short-lived scoped authority derives the remote subject and refuses audience, signature, expiry, revocation and scope failures by name.
owner: substrate
tags:
- identity
- remote
- security
relations:
- decomposes: epic:remote-serving
- depends_on: story:production-tls-control-listener
revision: 2
---
# Story: Hosted requests use the accepted trust envelope

## Outcome

A production network request is admitted only from an Identity-issued, short-lived, scoped trust envelope, and Substrate derives its subject and authority from the verified envelope rather than caller-written HTTP data.

## Coordination gate

Before code, an accepted Substrate design and the corresponding Identity audience/profile decision agree on the exact audience, key distribution, revocation semantics and scopes. The existing profile uses audience urn:b10x:substrate, EdDSA, a five-minute maximum lifetime, a sixty-second revocation objective, one audience, and observe/workspaces/exec scopes. Session authority requires a coordinated profile revision; Substrate must not invent it locally.

## Acceptance

The verifier fails closed on signature, issuer, audience, time, key, revocation and scope errors with named refusals. Tenant, actor, uid and subject headers or body fields never override verified claims. Keys rotate without restart and stale-key behavior is bounded. Authorization is checked per route and mutation before durable admission. Unix-socket requests continue to derive local:<uid> from kernel peer credentials and need no hosted envelope. Conformance vectors prove every negative case and that credential bytes and claims do not enter diagnostics or metrics.

## Out of Scope

Identity implementation, browser sessions, product roles, fleet placement and connector grants.
