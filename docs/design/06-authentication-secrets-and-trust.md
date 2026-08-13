# Design 06: authentication, secrets, and trust

**Status:** accepted v1 design · **Date:** 2026-08-13

Substrate needs enough local authority to protect a machine. It must not become Daemonloom's identity
provider, organization model, connector credential broker, or general authorization engine.

## 1. Authentication boundary

The founding wire prefers an owner-permissioned Unix-domain socket authenticated from OS peer
credentials mapped to a stable local subject. TCP uses a generated 256-bit bearer with stable
subject, actor label, explicit expiry, and coarse scopes: observe, workspaces, exec, sessions,
workloads, images, volumes, endpoints, and admin. A resource-family scope authorizes only that
family. `admin` authorizes token and daemon-configuration maintenance and never inherits a resource
scope. There is no unauthenticated personal-development mode. Reachable unauthenticated
listeners are refused at startup, and non-loopback control traffic requires TLS/mTLS or a configured
trusted tunnel.

Hosted deployments validate short-lived identity-issued service material through
[architecture ADR 0015 — Foundation services share one trust envelope](https://github.com/daemonloom/architecture/blob/main/adr/0015-foundation-trust-envelope.md).
Substrate stores only the accepted claims required for local admission and provenance; organization
membership and role evaluation remain outside.

## 2. Authorization split

Higher layers decide rich intent:

- connectors admits declared risks/effects and connector grants;
- cloud admits tenant placement and product quotas;
- agent products decide tool and run policy;
- autodev decides turn ownership and fleet assignment.

Substrate independently applies local token scope, resource ownership/addressing, capability,
limits, sandbox, exposure, and lease checks. A higher-layer permit cannot override them.
Every resource and operation-ledger key is scoped by deployment and authenticated subject. The
initiating platform principal, when present, is retained separately from the immediate service
actor through tamper-safe delegated context; caller-written identity strings are not trusted.

## 3. Secrets

Ordinary request JSON never contains secret values. Requests may reference a named daemon-configured
secret slot or an operation-scoped opaque handoff. The contract must distinguish:

- registry/source credentials used by the driver itself and immutably bound to configured
  destination constraints;
- workload secret slots mounted or delivered only to the selected workload;
- future connector-artifact credentials admitted and brokered by connectors;
- channel authorities used only for session establishment.

Secret material is absent from argv, ordinary environment, logs, events, ledger request hashes,
resource observations, and error bodies. If a driver cannot guarantee the chosen delivery channel,
it refuses before acquiring the value.

## 4. Deployment trust

Direct clients trust a configured daemon endpoint and credential. Hosted composition additionally
needs deployment identity, rotation, revocation, and authenticated registration. A connectors
satellite colocated with substrate is a separate service identity; process proximity does not create
implicit trust.

One daemon is one trust domain and, in v1, one tenant. Hosted placement must not multiplex mutually
untrusted tenants through one daemon. Operator-only unconfined or root-equivalent driver authority
uses a distinct credential and is disabled by default.

## V1 decisions and deferrals

1. **Local token lifecycle:** TCP tokens contain 256 random bits and a non-secret lookup prefix.
   Configuration stores SHA-256 digests, never bearer text; comparison is constant-time. Tokens have
   an explicit expiry, default 30 days and hard maximum 90 days, and may overlap during rotation.
   Revocation is an atomic configuration generation change, invalidates capability/auth caches, and
   applies before the next request. The generated bearer file is owner read/write only.
2. **Hosted identity:** RFC 0001 is accepted by architecture ADR 0015. Hosted material has a
   five-minute maximum lifetime and 60-second connected revocation bound; hosted auth remains phase
   7 and does not enter the minimum host slice.
3. **Secret slots:** no secret slot is served by the minimum host slice. Later host exec support uses
   a sealed Linux `memfd` passed at a declared child descriptor; only the slot-to-descriptor mapping
   (never the value) may appear in the shaped environment. Acquisition happens after all admission
   and dispatch-time checks, and the daemon closes its copy immediately after spawn. A driver that
   cannot prove sealing, descriptor isolation, and cleanup reports the capability absent.
4. **Brokered artifact secrets:** deferred with external connector artifacts. No generic opaque
   handoff exists until an attestation/supply-chain ADR accepts that implementation form.

Unix peer identity and local bearer identity are separate configured subjects. Neither can claim an
organization, platform principal, or delegated actor chain.
