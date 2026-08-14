# substrate-wire 0.2.0 development bundle

This directory is the machine-readable authority for substrate phase 3. It is deliberately a
development bundle: the bytes are deterministic and hash-covered, but there is no stable release,
OCI artifact, or signature yet. It preserves all twelve `0.1.0` routes and adds seven lifecycle
routes without changing the v1 API major. It does not claim full behavioral conformance with every
0.1 development vector: [`compatibility.json`](compatibility.json) names four predecessor errata.
`vectors/http/machinery-failure.json` corrects terminal failure retryability to `false`, and
`vectors/driver/crash-before-dispatch.json` corrects post-restart state to `unknown`.
`vectors/http/input-body-limit.json` corrects the transport fixture to a 2 MiB ceiling so an
exactly 1 MiB decoded file remains representable through base64 JSON. The 0.1 bytes remain
representable through base64 JSON. `vectors/http/write-limit.json` also makes a durably terminal
limit refusal non-retriable under the same operation id. The 0.1 bytes remain immutable; consumers
must select 0.2 to receive these safety corrections.

The bundle closes the phase-3 contract before server or driver types can become an accidental wire
specification. It contains the exact nineteen-operation registry, closed address/input/result
schemas, canonical v2 request-hash bytes, lifecycle and confinement invariant fixtures, complete
Design 04 threat coverage, exact HTTP/driver vectors, and the clean-room producer/runner protocol.
Run:

```text
python3 scripts/check-contract-bundle-0.2.0.py
```

The offline checker resolves and applies the bundled JSON Schemas; rejects duplicate keys,
unsafe paths, missing or extra routes and coverage; validates every exact response instance;
recomputes RFC 8785 integer-domain input bytes, the repository-authored rejected-number fallback,
strict duplicate-preserving form-query bindings, length-delimited tuples, and SHA-256 values; verifies positive and
negative state invariants; and checks every bundled path's media type, byte length, and digest. It
also classifies every JSON document under exactly one schema stored below `schemas/`. The
pinned Rust `jsonschema` validator validates every instance and every schema against the bundled
Draft 2020-12 meta-schema implementation without network retrieval; an unclassified JSON file
fails the bundle gate. Immutable rootless-URN schema identifiers are handled by deterministically
inlining their exact bundle-relative references before standards validation.
`bundle.json` excludes itself from its inner file list; the future OCI manifest digest covers it
without a recursive self-hash. `packaging.json` records the release recipe.

[`operations.json`](operations.json) is the route authority, [`hashing.json`](hashing.json) is the
request-hash authority, [`coverage.json`](coverage.json) is the exact conformance inventory, and
[`runner.json`](runner.json) defines the clean-room invocation and result protocol. The executable
vectors are producer obligations, not evidence that a host driver already exists or conforms.
Passing this checker proves bundle integrity, not runtime conformance. Stable publication still
requires independent execution, reproducible OCI packaging, signing, and digest pinning under
architecture ADR 0019.

Phase 3 adds bounded event pull and WebSocket push over one opaque source scope and durable
generation/sequence, explicit retention-gap recovery, and a non-keyed reconciliation-snapshot
control whose exact request body is `{}`. A snapshot contains the complete bounded current
workspace/exec set plus a bounded window of closed provenance events; metadata exposes the exact
resume cursor, history truncation, and partition counts. Explicit terminal exec retirement is the
only operation that frees its current-state/output quota. Explicit workspace and exec leases retain
their real authorizing operation, actor, and principal through renewal, conservative
clock-discontinuity expiry, and cleanup. The stream coalesces wake notifications and closes with a
typed boundary and pull cursor when catch-up or subscriber backpressure exceeds its bound.

The connectors projection manifest is connectors-owned and intentionally absent. Phase 6 will pin
this bundle digest from a manifest released beside the connector schema. The Git threat fixtures fix
the future destination boundary without serving Git. Sessions/stdin/PTY, workloads, images,
volumes, endpoints, Docker, Kubernetes, hosted identity, and connector projection remain absent
capabilities.
