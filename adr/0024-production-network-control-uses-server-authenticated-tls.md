---
status: accepted
date: 2026-09-01
---

# ADR 0024: production network control uses server-authenticated TLS

## Context

Substrate serves local callers over an owner-permissioned Unix socket and retains one explicitly
development-only static-bearer TCP listener. The latter can be placed on a private overlay by an
operator assertion, but it is plaintext, its bearer has no production lifetime or revocation
profile, and it cannot establish the channel binding required by network session authority.

A hosted caller needs to authenticate the daemon before it sends an authority document or any
control-plane bytes. Substrate also needs a production listener whose transport properties are
verified rather than inferred from deployment posture. Client identity remains an application
admission decision: requiring client certificates here would create a second principal and
revocation system beside the hosted trust envelope.

## Decision

Production network control uses a distinct HTTPS/WSS listener implemented with rustls and
tokio-rustls. It requires an explicit bind address, certificate chain and private-key file. TLS 1.3
and HTTP/1.1 are the initial served protocols; WebSocket upgrades use the same authenticated TLS
connection. There is no production plaintext fallback and no flag that disables certificate or
server-name verification in a client.

The server certificate authenticates the daemon. Mutual TLS is not required and a presented client
certificate does not become a Substrate subject. Remote subjects and route scopes come only from
the separately verified hosted trust envelope. The Unix listener continues to derive `local:<uid>`
from kernel peer credentials, and the development TCP listener remains a visibly separate posture.

The private-key path must name a non-empty regular file owned by the daemon's effective uid with no
group or other permission bits. The certificate chain must be a bounded non-empty regular file.
Neither path may be a symlink, and neither file's bytes, parse errors or derived key material enter
logs, events, metrics or refusal bodies. Missing, unsafe, mismatched, expired or not-yet-valid
identity material refuses startup before binding the listener.

SIGHUP reloads the certificate chain and key as one snapshot. A complete replacement is parsed and
matched before it is swapped into new connections; existing connections keep the snapshot under
which they were admitted. An invalid reload is rejected as `tls.reload-invalid`, retains the last
valid snapshot, and emits only the named condition and safe file identity—not certificate or key
bytes. Reload does not change authenticated subjects, capability generations or already durable
operations.

The listener uses the kernel peer address. It ignores `Forwarded`, `X-Forwarded-*`, PROXY protocol
and caller-written tenant, subject, actor or uid data. Deployments that need a reverse proxy must
preserve end-to-end TLS to this listener or make a later, explicit trusted-proxy decision.

The startup and reload boundary names `tls.listener-config-invalid`, `tls.private-key-unsafe`,
`tls.identity-invalid` and `tls.reload-invalid`. A plaintext attempt receives no HTTP response: it
fails the TLS handshake. Unknown roots, wrong server names and certificate time failures are client
verification failures and never become application admissions.

## Consequences

A remote caller can authenticate the daemon and carry HTTP and WebSocket control traffic without
turning transport certificates into product identity. Certificate rotation does not require a
daemon restart, and a broken rotation cannot partially replace the serving identity.

The daemon gains rustls and tokio-rustls dependencies and an operator-managed certificate lifecycle.
Hosted request admission, key distribution, scopes and revocation remain a separate coordinated
decision; enabling this listener without that verifier serves no application routes. ACME, ingress
controllers, service-mesh configuration, trusted forwarded addresses and client-certificate
principal mapping remain out of scope.
