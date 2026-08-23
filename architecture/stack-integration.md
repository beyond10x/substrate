# Stack integration

Substrate is useful both directly and through the Daemonloom control plane. The same resource and
failure semantics apply in every posture.

| System | Relationship to substrate | Boundary retained |
|---|---|---|
| connectors | exposes substrate operations under connector declarations/grants; may later request isolated execution of an attested connector artifact | connectors owns provider semantics, credentials, grants, audit, and artifact admission |
| identity | supplies trusted principal/service identity to hosted composition | substrate keeps only coarse local authentication and never becomes the organization/policy system |
| cloud | deploys, registers, selects, meters, and operates substrate instances | substrate owns resource and enforcement truth; cloud owns fleet/tenant composition |
| daemonloom/agent | may pin the daemon crate at its outer composition root, re-execute a private daemon child, then request bounded execution only through the Unix-socket contract | agent owns loops, tasks, tools, harnesses, model providers, and child lifecycle; substrate retains its process, protocol, authentication, and enforcement boundary |
| Flux | may map its guarded-IO and remote delegate seams onto the public API | Flux owns Flux-Lang, harness behavior, and error projection; substrate has no Flux dependency |
| autodev | may extract its proposed `Executor` port and implement it over workspaces, execs, leases, and snapshots | autodev owns turns, verification, scheduling, and coordinator refs |
| applications | consume direct or governed operations | applications never select driver internals or weaken isolation requirements |

## Connector path

When a connector grant governs a substrate action, connectors authenticates the rich principal,
admits declared risk/effects, selects a configured substrate connection, and sends one ordinary
substrate request. Substrate independently enforces its local token scope, capabilities, limits, and
isolation. Neither service treats the other's success as proof of its own checks.

A personal client may instead call substrate directly under architecture ADR 0013. That request is
locally authenticated, enforced, and observed, but it is not admitted by connector grants and does
not enter the durable platform audit.

Substrate events retain substrate resource and operation provenance when connectors republishes
them under
[architecture ADR 0017 — Connectors owns durable ingestion of substrate events](https://github.com/daemonloom/daemonloom/blob/e01ea676da18fb855814e7621514e0c98fc57c2c/architecture/adr/0017-substrate-event-ingestion.md).
Continuous PTY/tunnel bytes follow the byte-plane split: connectors brokers a short-lived session
authority under
[architecture ADR 0016 — Direct-byte establishment uses operation-scoped authority](https://github.com/daemonloom/daemonloom/blob/e01ea676da18fb855814e7621514e0c98fc57c2c/architecture/adr/0016-operation-scoped-session-authority.md),
while client and substrate exchange bytes directly.

## Hosted path

Cloud owns fleet inventory and tenant placement. It chooses an eligible registered deployment from
verified facts and policy, then invokes the same public contract. Substrate does not discover
tenants, schedule across machines, or phone home to a mandatory central service.

A private deployment may be colocated with a connectors satellite. Colocation is composition, not a
new substrate mode and not a reverse tunnel hidden inside substrate. Enrollment, outage, queue, and
containment semantics are fixed by
[architecture ADR 0018 — Connectors satellites federate outward under bounded authority](https://github.com/daemonloom/daemonloom/blob/e01ea676da18fb855814e7621514e0c98fc57c2c/architecture/adr/0018-connectors-satellite-federation.md).
