---
format: aep.planning-md/3
id: story:release-binaries
kind: story
status: draft
title: Each release attaches prebuilt daemon and CLI binaries
revision: 1
---
## Outcome

Each release attaches the daemon and CLI as x86_64 Linux archives with a keyless-signed
`SHA256SUMS`, built by the pinned Dockerfile builder.

## Decision

Operator, 2026-10-27: substrate should ship prebuilt daemon binaries, and a Docker image where one
fits (the daemon image already exists). Targets beyond x86_64 Linux are a later step.

## Acceptance

1. `substrate-daemon-<version>-x86_64-unknown-linux-gnu.tar.gz`,
   `b10x-substrate-<version>-x86_64-unknown-linux-gnu.tar.gz`, `SHA256SUMS` and
   `SHA256SUMS.sigstore.json` are attached to the GitHub release.
2. Binaries need glibc 2.35 or older and start in `ubuntu:22.04`.
3. The archived daemon equals the image's before any tag is pushed.
4. `SHA256SUMS` is signed and verified with the release workflow identity before the release is
   announced.
5. `xtask/tests/release_workflow.rs` holds each of these offline.
