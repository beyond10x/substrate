# substrate-wire 0.1.0 development bundle

This directory is the first machine-readable authority for substrate phase 2. It is deliberately a
development bundle: the bytes are deterministic and hash-covered, but there is no stable release,
OCI artifact, or signature yet.

The bundle closes the phase-2 contract before server or driver types can become an accidental wire
specification. It contains the exact twelve-operation registry, closed address/input/result schemas,
canonical request-hash bytes, lifecycle and confinement invariant fixtures, complete Design 04
threat coverage, exact HTTP/driver vectors, and the clean-room producer/runner protocol. Run:

```text
python3 scripts/check-contract-bundle.py
```

The dependency-free checker resolves and applies the bundled JSON Schemas; rejects duplicate keys,
unsafe paths, missing or extra routes and coverage; validates every exact response instance;
recomputes RFC 8785 input bytes, length-delimited tuples, and SHA-256 values; verifies positive and
negative state invariants; and checks every bundled path's media type, byte length, and digest.
`bundle.json` excludes itself from its inner file list; the future OCI manifest digest covers it
without a recursive self-hash. `packaging.json` records the release recipe.

[`operations.json`](operations.json) is the route authority, [`hashing.json`](hashing.json) is the
request-hash authority, [`coverage.json`](coverage.json) is the exact conformance inventory, and
[`runner.json`](runner.json) defines the clean-room invocation and result protocol. The executable
vectors are producer obligations, not evidence that a host driver already exists or conforms.
Runtime scaffolding may now begin; stable publication still requires independent execution,
reproducible OCI packaging, signing, and digest pinning under architecture ADR 0019.

The connectors projection manifest is connectors-owned and intentionally absent. Phase 6 will pin
this bundle digest from a manifest released beside the connector schema.

Phase 2 serves only the endpoints in `operations.json`. The phase-6 Git threat fixtures fix the
future destination boundary without serving Git in phase 2. Events, leases, sessions, workloads,
images, volumes, endpoints, Docker, Kubernetes, hosted identity, and connector projection remain
absent capabilities.
