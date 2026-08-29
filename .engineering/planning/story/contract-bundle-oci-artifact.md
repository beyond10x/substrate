---
format: aep.planning-md/1
id: story:contract-bundle-oci-artifact
kind: story
status: draft
title: The 0.4.0 contract bundle is a signed, digest-pinned OCI artifact
summary: Design 07 fixes the packaging shape; consumers copy bundle bytes by hand today.
owner: substrate
tags:
- contracts
- release
relations:
- decomposes: epic:release-hardening
- depends_on: story:signed-daemon-image
revision: 2
---
# Story: The 0.4.0 contract bundle is a signed, digest-pinned OCI artifact

## Outcome

A consumer pins the wire contract by OCI digest instead of copying a directory tree.

## Context

`docs/plan/04-direct-byte-plane.md` § *Slice C* says Agent "consumes an exact independently
verified copy"; the copy is made by hand. `docs/design/07-specification-and-conformance.md`
§ *Bundle* already fixes the packaging: `bundle.json` lists media type, byte length and digest of
every bundle path except itself, and the outer OCI manifest digest pins `bundle.json`. Depends on
`story:signed-daemon-image` for the release workflow it publishes through.

## Acceptance

`scripts/package-contract-bundle.py <version>` produces a deterministic OCI layout from
`contracts/substrate-wire/<version>/` without writing into `contracts/`, two runs yield
byte-identical manifests, and the release workflow publishes and signs it at
`ghcr.io/beyond10x/b10x-substrate-wire:<bundle-version>` with the digest in `CHANGELOG.md`.

Evidence that satisfies it:

- `scripts/test_package_contract_bundle.py`: identical manifests on two runs; a one-byte change in
  any bundle file changes the manifest digest — written failing-first against a stub that embeds
  a timestamp;
- `bundle.json` matches design 07's shape and a checker asserts each digest and byte length;
- `check-contract-bundle-0.4.0.py` and every earlier checker stay green (invariant 6: the packager
  reads the bundle; the bundle does not learn about the packager);
- the artifact is annotated `development`; publication does not make the bundle stable (atlas ADR
  0019 governs contract release);
- `bash scripts/gate.sh` exits 0.
