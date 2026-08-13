# substrate-wire 0.1.0 development bundle

This directory is the first machine-readable authority for substrate phase 2. It is deliberately a
development bundle: the bytes are deterministic and hash-covered, but there is no stable release,
OCI artifact, or signature yet.

The bundle defines the common envelopes and an initial subset of minimum-host conformance vectors
before server or driver types can become an accidental wire specification. Route-specific schemas,
the complete Design 04 vector inventory, clean-room execution, deterministic tar proof, OCI
publication, and signing remain release blockers. Run:

```text
python3 scripts/check-contract-bundle.py
```

The checker parses every JSON file, rejects unmanifested or missing files, verifies every bundled
path's media type, byte length, and SHA-256, and checks vector identity and shape. `bundle.json`
excludes itself from its inner file list; the future OCI manifest digest covers it without a
recursive self-hash. `packaging.json` records the release recipe. This hand-authored development
snapshot is readable source material; the release generator must emit RFC 8785 authority bytes and
then package those exact bytes under architecture ADR 0019 rather than interpreting Markdown.

The connectors projection manifest is connectors-owned and intentionally absent. Phase 6 will pin
this bundle digest from a manifest released beside the connector schema.

Phase 2 serves only the endpoints listed in Design 07. Git, events, leases, sessions, workloads,
images, volumes, endpoints, Docker, Kubernetes, hosted identity, and connector projection remain
absent capabilities.
