# Dependency rules

Dependencies point from products and composition toward substrate. Substrate never links a
consumer's implementation.

## Allowed direction

```text
cloud / connectors / agent / Flux / autodev / products ──▶ substrate API
substrate service ──▶ substrate domain and driver ports
host / Docker / Kubernetes adapters ──▶ substrate driver ports
```

Protocol clients may be generated from a released substrate specification, but compatibility never
relies on sibling checkout paths.

A product composition root may pin and compile the `substrate-daemon` crate solely to bootstrap a
separate daemon child process from a single-file distribution. Product operations must still use
the public Unix-socket contract; importing daemon application or host-driver APIs into product
session, task, tool, or workflow code is forbidden. Packaging the entrypoint does not collapse the
process or trust boundary.

## Forbidden direction

```text
substrate ─X─▶ Flux crates or types
substrate ─X─▶ agent loops or harness SDKs
substrate ─X─▶ connector catalog/runtime implementations
substrate ─X─▶ identity/cloud implementations
substrate ─X─▶ autodev or product code
domain ─X─▶ HTTP, Docker, Kubernetes, persistence, or telemetry libraries
```

## Cross-foundation seams

- Connectors may declare substrate as a first-party provider through a deterministic translation of
  a released substrate bundle plus a connectors-owned manifest. Substrate does not need connectors
  to serve its direct API.
- Hosted authentication may validate identity-issued material through a narrow stable protocol;
  substrate does not import the identity domain implementation.
- Cloud may provision, register, meter, and select deployments through public APIs. It cannot add a
  cloud-only substrate rule.
- A future connector artifact may be executed through substrate only as generic bounded work after
  connectors makes its separate attestation decision.
- Flux and autodev own their adapters and vocabulary projection.

Future implementation CI must classify every package by layer and reject forbidden dependency
edges, including development and build dependencies.
