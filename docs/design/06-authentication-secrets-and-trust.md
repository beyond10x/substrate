# Design 06: authentication, secrets, and trust

**Status:** draft for review · **Date:** 2026-08-13

Substrate needs enough local authority to protect a machine. It must not become Daemonloom's identity
provider, organization model, connector credential broker, or general authorization engine.

## 1. Authentication boundary

The founding wire uses bearer tokens configured at the daemon with an actor label and coarse scopes:
observe, workspaces, exec, workloads, images, and admin. Loopback still authenticates unless an
explicit personal-development mode is later accepted. Reachable unauthenticated listeners are
refused at startup.

Hosted deployments may later validate short-lived identity-issued service material through a stable
protocol or standard. Substrate stores only the claims required for local admission and provenance;
organization membership and role evaluation remain outside.

## 2. Authorization split

Higher layers decide rich intent:

- connectors admits declared risks/effects and connector grants;
- cloud admits tenant placement and product quotas;
- agent products decide tool and run policy;
- autodev decides turn ownership and fleet assignment.

Substrate independently applies local token scope, resource ownership/addressing, capability,
limits, sandbox, exposure, and lease checks. A higher-layer permit cannot override them.

## 3. Secrets

Ordinary request JSON never contains secret values. Requests may reference a named daemon-configured
secret slot or an operation-scoped opaque handoff. The contract must distinguish:

- registry/source credentials used by the driver itself;
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

## Decisions required before implementation

1. Personal-mode authentication requirements.
2. Token storage, hashing, rotation, and revocation behavior.
3. Hosted service-identity mechanism and stable validation protocol.
4. Secret slot lifecycle and Linux delivery primitive.
5. Brokered operation-secret handoff for a future attested artifact; this remains deferred with the
   connector artifact decision.
