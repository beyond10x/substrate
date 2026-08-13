# Repository status

**Observed:** 2026-08-13

## Current state

| Area | State | Next proof |
|---|---|---|
| Repository | private `daemonloom/substrate`; bot-authored `main` is synchronized | keep visibility, authorship, and portable-document invariants enforced |
| Boundary | accepted: standalone, generic execution data plane, Flux-free | keep all design documents consistent with ADRs 0001 and 0002 |
| Wire contract | detailed founding draft | close the questions named by the design-closure plan |
| Drivers | host, Docker, and later Kubernetes intent documented | accept the driver/capability model before selecting implementation crates |
| Security | principles and known threats documented | complete isolation guarantees and secret handoff design |
| Stack integration | ownership mapped across connectors, identity, cloud, agent, Flux, and autodev | agree direct, governed, and hosted deployment postures |
| Implementation | intentionally absent | begin only after the design-closure gate is accepted |

## Repository facts

- No `Cargo.toml`, source tree, implementation crate, generated API, or container artifact exists.
- No Flux package, type, protocol, or checkout is required.
- The existing contract is preserved and curated rather than replaced.
- The first planned implementation is a minimum host slice; Docker and Kubernetes do not block it.

## External dependencies

Substrate has no source dependency on another Daemonloom repository. Cross-repository compatibility
will use stable wire contracts and conformance fixtures, never sibling path dependencies.
