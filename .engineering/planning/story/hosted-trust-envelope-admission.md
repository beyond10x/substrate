---
format: aep.planning-md/1
id: story:hosted-trust-envelope-admission
kind: story
status: implemented
title: Hosted requests use the accepted trust envelope
summary: Identity-issued short-lived scoped authority derives the remote subject and refuses audience, signature, expiry, revocation and scope failures by name.
owner: substrate
tags:
- identity
- remote
- security
- wave/remote-foundation-01
relations:
- decomposes: epic:remote-serving
- depends_on: story:production-tls-control-listener
revision: 7
---
# Story: Hosted requests use the accepted trust envelope

## Outcome

A production network request is admitted only from an Identity-issued, five-minute, audience- and scope-bound opaque access credential. Substrate derives its subject and authority from Identity's current resolved authority rather than caller-written HTTP data.

## Coordination decision

Identity decision 0004 provides the corresponding released profile: deployment-registered opaque audiences and scopes, five-minute verifier-only access credentials, exact-audience online resolution and total logout revocation. ADR 0026 fixes Substrate's relying-party side: exact audience `urn:b10x:substrate`, scopes `observe`, `workspaces` and `exec`, explicit HTTPS trust roots, no redirect or proxy, and fail-closed per-request resolution. The earlier proposed EdDSA/key-distribution profile is not implemented because it is not Identity's released seam.

## Acceptance

The production TLS listener refuses a missing credential, an invalid/resolved authority, a missing route scope, or Identity unavailability by a safe name before durable admission. Issuer, audience, time window, five-minute lifetime, subject, tenant, actor and canonical scope bytes are validated locally after resolution. Tenant, actor, uid and subject headers or body fields never override verified claims. Revocation takes effect on the next request because no authority cache exists. Unix-socket requests continue to derive `local:<uid>` from kernel peer credentials and never contact Identity. Conformance tests prove negative cases, exact per-route scopes, revocation, spoof resistance and that credentials and authority documents do not enter diagnostics or metrics.

## Out of Scope

Identity implementation, browser sessions, product roles, fleet placement, connector grants and signed-token key distribution.

## Implementation evidence

The implementation is coordinated by accepted ADR 0026 and contract bundle `substrate-wire/0.13.0`. The production TLS listener resolves each opaque `identity_access_v1_` credential against Identity over TLS 1.3 with explicit trust roots and no proxy, redirect or authority cache. It derives non-reversible tenant-bound subject and actor references, removes caller authority before dispatch, and enforces the exact `observe`, `workspaces` and `exec` route scopes.

Black-box tests of the shipped daemon prove valid admission, missing and invalid credentials, insufficient scope before durable operation creation, Identity unavailability, immediate revocation, spoofed authority headers, WebSocket admission, certificate rotation and diagnostic redaction. Unit tests cover origin and trust-root validation, closed resolved-authority parsing, exact time and lifetime bounds, credential shape, scope mapping and subject derivation. `bash scripts/gate.sh` passed with all 13 contract bundles and 2,820 classified JSON documents; `bash scripts/delegated-lane.sh` passed under delegated cgroup v2 confinement.
