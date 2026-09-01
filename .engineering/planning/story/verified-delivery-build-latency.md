---
format: aep.planning-md/1
id: story:verified-delivery-build-latency
kind: story
status: implemented
title: Compile and validate releases without duplicate build work
summary: Reduce gate and release wall time while preserving exact shipped-binary, contract, signing, and branch-protection evidence.
tags:
- ci
- performance
- release
relations:
- derived_from: epic:release-hardening
revision: 6
---
# Compile and validate releases without duplicate build work

## Observed baseline

Release run `33498193209` took 12m15s in its publication job. Its daemon image step took 5m26s and its MCP image step took another 4m22s. The two Dockerfiles used different Cargo target-cache identities and therefore compiled the shared Rust graph twice. Main gate run `33497345920` took 9m27s: 68s restored a 4.0 GiB broad Cargo cache, 59s saved it, the release test build took 3m30s, and sequential bundle plus JSON checks took about 1m45s.

## Required outcome

- One pinned container builder stage compiles the daemon and MCP binaries together; named minimal runtime targets keep the daemon and stdio-only MCP image boundaries distinct.
- The release still runs the clean-room daemon vectors against the extracted shipped daemon binary and both MCP stdio journeys against the extracted binary and final container image.
- Current bundle fixed-point checks and closed JSON classification execute concurrently with bounded worker counts and deterministic reporting.
- CI caches dependency artifacts rather than uploading the complete workspace target tree, while keeping every gate command and refusal.
- Repository tests assert the shared build boundary, minimal MCP image, digest signing, anonymous readback, and write-once behavior.

## Evidence to close

A green full local gate, a green pull-request Full gate with step timings, and a green protected-main Full gate. The first run establishes correctness; later runs establish cache effectiveness.

## Additional profile reuse

Clippy now runs with `--release`, matching the immediately preceding release-profile test build. The repository has no `cfg(debug_assertions)` implementation branch, so this preserves the linted source and target set while avoiding a second dependency-profile graph. The first local release-profile Clippy run took 16.8s; the previous hosted development-profile step took 49.4s.

## Hosted result

The protected-main seed run `33502295385` was green in 12m02s and saved a 578,678,197-byte cache. Protected-main run `33503894885` restored that cache in about four seconds, reused `cargo-about`, and completed green in 7m09s. Against baseline main run `33497345920` at 9m27s, the steady-state wall time fell by 2m18s (24%). The remaining dominant step is the exact release-profile workspace test build at 4m07s; no test, refusal, contract, security, licence, or shipped-binary check was removed.
