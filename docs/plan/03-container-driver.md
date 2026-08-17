# Plan 03: container driver

**Status:** deferred until host conformance · **Date:** 2026-08-13

Docker is the second driver because it tests whether the contract genuinely separates domain
semantics from execution mechanics. It is not the definition of substrate isolation.

## Entry criteria

- The minimum host slice passes public-wire and driver conformance tests.
- Driver ports contain no host library types.
- Capability predicates and applied-enforcement observations are stable.
- The security design states plainly which Docker authorities are root-equivalent.

## Planned proof

- Run the same workspace/exec conformance journey using container-backed execution.
- Add image pull/inspection and workload lifecycle only through accepted contract operations.
- Record image digests and applied container isolation as observations.
- Prove unsupported or unsafe features as `unserved`/`refused`, never host fallback.
- Demonstrate that a client does not branch on Docker-specific response types.

## Exit criteria

- Host and Docker pass the same applicable conformance fixtures.
- Driver-specific capabilities explain every intentional difference.
- Docker socket and daemon privileges are visible deployment facts.
- No container-specific rule leaks into connector, agent, Flux, or autodev contracts.
