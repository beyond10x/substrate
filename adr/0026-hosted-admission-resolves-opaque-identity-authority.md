---
status: accepted
date: 2026-09-01
---

# ADR 0026: hosted admission resolves opaque Identity authority

## Context

ADR 0024 provides a production HTTPS/WSS listener but deliberately admits no application request
until a hosted identity profile exists. Identity now publishes that profile: a five-minute opaque
access credential is minted for one exact deployment-registered audience and scope set, while
`GET /v1/access-authority` returns the current authority only when the credential is present,
unexpired and unrevoked. Identity stores only its SHA-256 verifier and logout revokes every
outstanding access credential for the subject.

The older Substrate design expected an EdDSA document, key distribution and a bounded stale-key
cache. That is not the released Identity seam. Adding a second signed-token authority here would
duplicate credential lifecycle, weaken immediate revocation and make Identity compile a Substrate
profile. Identity decision 0004 instead makes audience and scope bytes opaque deployment data; the
relying party owns their meaning and final authorization (`identity/docs/decisions/0004-agnostic-relying-party-registration.md`
in the sibling repository).

## Decision

The production listener requires an Identity origin and an explicit CA bundle in addition to its
server certificate. The origin is an exact HTTPS origin with no user information, query, fragment
or non-root path. Substrate connects directly to that origin, verifies its server name against the
configured CA roots, sends no request through a proxy and follows no redirect. Startup refuses an
invalid origin or trust bundle before binding.

Every production request must present one bounded `identity_access_v1_` bearer. Substrate resolves
it online at `GET /v1/access-authority`, sending the exact audience `urn:b10x:substrate` in
`x-b10x-audience`. The resolver has fixed connect and total deadlines and a 64 KiB response limit.
Only a successful closed authority document is considered; any other status, malformed document,
issuer or audience mismatch, invalid time window, lifetime over five minutes, invalid identity
field or unregistered scope refuses admission. Redirects are never followed.

The resolved authority derives the Substrate subject from the tenant and subject together, retains
the immediate actor separately, and cannot be overridden by an HTTP header or body field. The
bearer is removed before dispatch. Credential bytes, authority documents and resolution errors do
not enter logs, events, diagnostics or metrics.

Identity's registered scopes use the existing Substrate contract vocabulary:

| scope | admitted route families |
|---|---|
| `observe` | machine, metrics, events, reconciliation snapshots and operation observations |
| `workspaces` | workspace resources, files and workspace leases |
| `exec` | executions, execution leases and pipe/PTY sessions |

Authorization runs before every handler and therefore before durable operation admission. A valid
credential without the route's exact scope is refused. Unknown routes still require a valid
credential but no resource scope, so authentication does not become a route-discovery oracle.
Unix-socket admission remains kernel-derived and does not contact Identity; the development TCP
listener remains its distinct loopback-only static-bearer posture.

There is no key cache to rotate or become stale. Identity answers current durable state on every
request, so a completed logout or explicit revocation is effective on the next request. An
in-flight request keeps the authority already admitted for that request only. Identity
unavailability never falls back to cached authority, a caller-written identity or the development
bearer.

The pre-handler boundary names four safe failures: `auth.credential-absent`,
`auth.authority-invalid`, `auth.scope-denied` and `auth.authority-unavailable`. Authentication
failures intentionally do not distinguish expired, revoked or unknown credential values to the
caller. Startup names `auth.listener-config-invalid` and `auth.trust-roots-invalid`.

## Consequences

Hosted requests use Identity's released opaque-credential lifecycle and exact audience registry,
including current revocation, without embedding Identity implementation or product policy.
Identity availability is now on the production request path; failure is bounded and fail-closed.
Deployments register `urn:b10x:substrate` with the three scopes before enabling the listener.

The authentication refusals and hosted profile are wire-visible contract material, so the
implementation cuts the next development bundle and leaves every earlier bundle untouched.
