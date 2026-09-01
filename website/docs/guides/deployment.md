---
title: Deployment postures
description: How reachability and trust change without weakening Substrate's resource semantics.
---

# Put the same contract behind an explicit trust boundary

A deployment posture changes reachability, authentication, storage, and operations. It does not
change what a workspace, exec, operation, refusal, or observation means.

## Personal

The current starting point is an owner-permissioned Unix socket:

- the operator explicitly allows one or more numeric user IDs;
- each request derives its local subject from kernel peer credentials;
- the daemon governs one owner trust domain;
- callers may connect directly without a platform network hop.

This is the posture used in [getting started](../getting-started.md).

## Organization and hosted shapes

Private-network and hosted compositions require deployment identity, scoped authority, encrypted
transport, rotation, and operational controls around the same data-plane contract. Those shapes do
not imply that every hosted trust feature is implemented today.

One daemon remains one trust domain. Do not multiplex mutually untrusted tenants through the same
daemon merely because a higher layer can attach different account labels.

## Production TLS transport

Current source can terminate TLS 1.3 for HTTP and WebSocket control traffic with an explicit server
identity:

```console
substrate-daemon \
  --socket /run/substrate/local.sock \
  --state /var/lib/substrate/state.sqlite \
  --workspaces /var/lib/substrate/workspaces \
  --deployment edge-01 \
  --tls-listen 0.0.0.0:8443 \
  --tls-certificate-chain /run/substrate-tls/chain.pem \
  --tls-private-key /run/substrate-tls/key.pem
```

The certificate chain and key must be non-empty regular files rather than symlinks. The key must
belong to the daemon's effective user and carry no group or other permission bits. Startup validates
the leaf certificate's time window and its agreement with the key before binding.

To rotate the identity, replace both files completely and send the daemon SIGHUP. New connections
use the replacement only after the complete pair validates; existing connections keep the snapshot
under which they connected. If a replacement is invalid, new connections continue to use the last
valid pair and the daemon emits the safe condition `tls.reload-invalid` without certificate or key
bytes.

The listener authenticates the daemon, not the caller. Hosted caller admission is not implemented
yet, so application requests currently receive a pre-admission `503` and no remote subject is
derived. There is no production plaintext fallback, mutual-TLS identity mapping, trusted forwarded
address, or verification-disable switch.

## Static-bearer TCP is development-only

The currently implemented TCP transport is deliberately restricted. It requires all of:

- an explicit development-only acknowledgement;
- an explicit private-overlay acknowledgement;
- a loopback listen address;
- a bounded bearer file;
- deployment-owned subject and actor bindings.

It is not a production hosted trust envelope and cannot be bound to a non-loopback address. Use it
only for local development.

## Rules that do not change by posture

- Never bind a reachable unauthenticated control listener.
- Never infer a capability from a posture name.
- Never weaken a requested sandbox because the selected machine lacks enforcement.
- Keep fleet scheduling, membership, rich grants, product quotas, and billing outside the daemon.
- Keep continuous session bytes off an ordinary governance or invocation proxy.
- Scope resources and operation IDs to the authenticated subject.

## Deployment review

Before making a daemon reachable beyond its owner, answer:

1. Which trust domain owns the machine?
2. How does a connection prove its subject and immediate actor?
3. How are credentials bounded, rotated, and removed?
4. Which network can reach the control listener?
5. Which execution capabilities did this exact backend verify?
6. Where do event retention, state storage, and workspace data live?
7. What refuses startup if any required trust control is absent?

If any answer relies on “the surrounding network is probably safe,” keep the Unix socket posture.
