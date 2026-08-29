---
format: aep.planning-md/1
id: story:contract-bundle-oci-artifact
kind: story
status: active
title: The 0.4.0 contract bundle is a signed, digest-pinned OCI artifact
summary: Design 07 fixes the packaging shape; consumers copy bundle bytes by hand today.
owner: substrate
tags:
- contracts
- release
relations:
- decomposes: epic:release-hardening
- depends_on: story:signed-daemon-image
revision: 5
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

## Progress — 2026-08-29 (packager half)

- `scripts/package-contract-bundle.py <version> --out <dir>`: OCI Image Layout (`oci-layout`,
  `index.json`, `blobs/sha256/…`); config **is** `bundle.json` verbatim, media type
  `application/vnd.b10x.substrate-wire.bundle.v1+json`, so the manifest digest pins
  `bundle.json` (design 07 § *Bundle*); one layer per bundle file, layer digest = the `sha256`
  already in `bundle.json`; annotations `org.opencontainers.image.version`,
  `dev.b10x.contract.status=development`, `ref.name` on the index entry. Refuses `--out` inside
  `contracts/`, non-empty `--out` without `--force`, symlinks, a file set disagreeing with
  `bundle.json`.
- `scripts/test_package_contract_bundle.py`: 14 tests; determinism test written failing-first
  against a timestamp-embedding stub (`FAILED (failures=1)`), then green. Two runs on `0.4.0` →
  `sha256:f94d15fc116587d991aab6de5628f6ee5baf872af4e7c79d2a35e2b17a8485c4`, `diff -r` empty.
  `oras manifest fetch --oci-layout` resolves the same digest; `oras pull` round-trip matches
  `contracts/substrate-wire/0.4.0` byte for byte.
- `check-contract-bundle-0.4.0.py` → exit 0; `git status contracts/` → clean (invariant 6).
- Wired into the gate at `scripts/gate.sh:24`.
- Noted gap: `packaging.json` declares a `posix-tar` source archive; this layout is
  file-per-layer. Same bytes, per-file digests exposed; the release story decides whether the
  tar form is also wanted.
- **Open:** the publish/sign half — `release.yml`, `ghcr.io/beyond10x/b10x-substrate-wire`,
  cosign, digest in `CHANGELOG.md` — waits on `story:signed-daemon-image`.
