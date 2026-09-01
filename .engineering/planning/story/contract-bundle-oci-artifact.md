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
- wave/remote-foundation-01
relations:
- decomposes: epic:release-hardening
- depends_on: story:signed-daemon-image
revision: 13
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

`cargo xtask package-bundle <version>` produces a deterministic OCI layout from
`contracts/substrate-wire/<version>/` without writing into `contracts/`, two runs yield
byte-identical manifests, and the release workflow publishes and signs it at
`ghcr.io/beyond10x/b10x-substrate-wire:<bundle-version>` with the digest in `CHANGELOG.md`.

Evidence that satisfies it:

- `xtask/src/package.rs` tests: identical manifests on two runs; a one-byte change in
  any bundle file changes the manifest digest — written failing-first against a stub that embeds
  a timestamp;
- `bundle.json` matches design 07's shape and a checker asserts each digest and byte length;
- `check-contract-bundle-0.4.0.py` and every earlier checker stay green (invariant 6: the packager
  reads the bundle; the bundle does not learn about the packager);
- the artifact is annotated `development`; publication does not make the bundle stable (atlas ADR
  0019 governs contract release);
- `bash scripts/gate.sh` exits 0.

## Progress — 2026-08-29 (packager half)

- `cargo xtask package-bundle <version> --out <dir>`: OCI Image Layout (`oci-layout`,
  `index.json`, `blobs/sha256/…`); config **is** `bundle.json` verbatim, media type
  `application/vnd.b10x.substrate-wire.bundle.v1+json`, so the manifest digest pins
  `bundle.json` (design 07 § *Bundle*); one layer per bundle file, layer digest = the `sha256`
  already in `bundle.json`; annotations `org.opencontainers.image.version`,
  `dev.b10x.contract.status=development`, `ref.name` on the index entry. Refuses `--out` inside
  `contracts/`, non-empty `--out` without `--force`, symlinks, a file set disagreeing with
  `bundle.json`.
- 14 tests; determinism test written failing-first
  against a timestamp-embedding stub (`FAILED (failures=1)`), then green. Two runs on `0.4.0` →
  `sha256:f94d15fc116587d991aab6de5628f6ee5baf872af4e7c79d2a35e2b17a8485c4`, `diff -r` empty.
  `oras manifest fetch --oci-layout` resolves the same digest; `oras pull` round-trip matches
  `contracts/substrate-wire/0.4.0` byte for byte.
- `check-contract-bundle-0.4.0.py` → exit 0; `git status contracts/` → clean (invariant 6).
- Gated by `cargo test --workspace` (`scripts/gate.sh:15`), not by a gate line of its own.
- The `posix-tar` archive `packaging.json` declares is now the last layer (media type
  `application/vnd.b10x.substrate-wire.bundle.tar`, title `0.4.0.tar`): ustar, directory entries
  0755 ahead of files 0644, uid/gid 0, empty names, bytewise order, every mtime =
  `SOURCE_DATE_EPOCH` = author seconds of the last commit touching the bundle (`1787605082`,
  commit `4b5d411`; `--source-date-epoch` overrides, absent both → exit 2). Per-file layers and the
  config are byte-identical to before; the manifest digest moves by design to
  `sha256:3758e80bc39f1eb03b15c69410608c9ef1d2ba8095c7e707c6988dbb5894ab00`
  (archive `sha256:91fb5524…`, 880,640 bytes). 21 tests; `tar -xf` of the blob reproduces
  `contracts/substrate-wire/0.4.0` byte for byte. On a shallow CI checkout the epoch would be the
  tip's author time — pin `--source-date-epoch` there.
- **Open:** the publish/sign half — `release.yml`, `ghcr.io/beyond10x/b10x-substrate-wire`,
  cosign, digest in `CHANGELOG.md` — waits on `story:signed-daemon-image`.

## Correction — 2026-08-30

The two Python names this story was written against no longer exist. `ef22de0` ("the bundle
packager is cargo xtask package-bundle, and its Python is gone") ported the packager and deleted
`scripts/package-contract-bundle.py` and `scripts/test_package_contract_bundle.py`. Acceptance and
the progress notes above now name the Rust verb; the behaviour they assert is unchanged, and the
`0.4.0` manifest digests recorded above were observed against the Python and have not been
re-observed against the verb.

Coverage is not lost in the move: `xtask/src/package.rs` carries 21 `#[test]`s (was 21 Python
tests), and `cargo test --workspace` at `scripts/gate.sh:15` runs them. There is no
`package-bundle` line in `scripts/gate.sh` and none is needed.

**Still open, unchanged:** the publish/sign half — `release.yml`,
`ghcr.io/beyond10x/b10x-substrate-wire`, cosign, digest in `CHANGELOG.md`. It waits on
`story:signed-daemon-image`, which is a draft. This story cannot reach `implemented` until that
one is designed and built.

## Unblocked — 2026-08-30

The paragraph above says this waits on `story:signed-daemon-image`, "which is a draft". It is
`implemented` (closed at `9131c95`). `.github/workflows/release.yml` exists, has run, and published
`ghcr.io/beyond10x/b10x-substrate-daemon:0.2.3` at
`sha256:ab10158266b579d705ce8422c7d2a6e783cde950d30e100f61ca6befc4d0beda`, keyless-signed and
`cosign verify`-ed (run 33304493276).

**Nothing blocks this story.** What remains is its own second half: a bundle publish-and-sign job in
`release.yml`, publishing `ghcr.io/beyond10x/b10x-substrate-wire:<bundle-version>` from
`cargo xtask package-bundle`, with the digest in `CHANGELOG.md` and the artifact annotated
`development` — a development bundle does not become a stable contract by being published (atlas
ADR 0019).

One thing the daemon release proved that this story must plan around: the workflow's final step
cannot push to `main`, because `main` is protected and requires the `Full gate` check, which a
direct push can never satisfy. The daemon digest landed by pull request instead
(`68d226f`). A bundle digest will need the same route or a different one, and the story should say
which before the job is written.

## Implementation — 2026-09-01 (publish/sign half)

- `.github/workflows/release.yml` explicitly pins current development bundle `0.12.0`; a successor
  must move that reviewable pin rather than being selected by directory order at release time.
- One globally serialized release job refuses an existing GitHub release or daemon image tag. A
  contract-bundle tag is reused only when it already resolves to the deterministic local digest; a
  mismatched digest, authentication failure, transport failure or ambiguous registry answer refuses.
  The canonical bundle tag is never an overwrite target.
- The job runs `cargo xtask package-bundle`, checks the manifest digest and
  `dev.b10x.contract.status=development` annotation locally, and uses pinned ORAS 1.3.3 to copy the
  exact OCI Image Layout to `ghcr.io/beyond10x/b10x-substrate-wire:<bundle-version>` when absent.
  It resolves the remote tag back to the packaged digest, checks the development annotation, and
  proves anonymous retrieval before mutating the daemon-image tag; a visibility failure is retryable
  without overwriting either artifact.
- Cosign signs the bundle by digest with the tag-triggered workflow's OIDC identity and verifies the
  exact certificate identity and GitHub Actions issuer before `gh release create`. Release notes
  carry both immutable digests, both verification commands and the explicit non-stability statement.
- Protected `main` is not pushed or bypassed. After both signatures verify, the workflow emits the
  exact daemon and bundle digest lines for a workstation's `b10x-bot` pull request, which receives
  the required `Full gate` check.
- `xtask/tests/release_workflow.rs` fails closed offline on the current-bundle pin, exact-layout copy,
  two write-once-or-identical-reuse checks, anonymous-access ordering, digest signing/verification order, development wording, protected-main
  route and commit-pinned actions. A local ORAS layout-to-layout round trip reproduced manifest
  `sha256:dd901e848c821aca7d55f7b8cf5ee893e1d99a1428b348e32e7ed1045a375319`; `contracts/` stayed
  clean.
- **Live evidence remains intentionally absent:** this implementation task forbids publishing or
  cutting a tag. `ghcr.io/beyond10x/b10x-substrate-wire:0.12.0` was still not found on 2026-09-01.
  The story stays `active` until an eligible release observes the remote digest and verified
  signature and a fully gated bot pull request records that observed digest in `CHANGELOG.md`.

## Live release evidence — 2026-09-01

- Annotated tag `0.4.1` names fully gated protected-main merge `961be39`; release preflight passed against gate run `33457279192`.
- The first publication attempt copied deterministic bundle `0.12.0` to public GHCR digest `sha256:dd901e848c821aca7d55f7b8cf5ee893e1d99a1428b348e32e7ed1045a375319`, then the authenticated immediate ORAS readback failed before signing. A failed-job rerun proved the tag exists at the identical digest but hit the same readback boundary; no daemon image or GitHub release was announced.
- The follow-up changes GHCR login identity from the repository owner to `github.actor` and makes the post-publish proof explicitly anonymous with a bounded retry. Offline workflow tests pin both properties. Live signing, anonymous ORAS proof and the protected-main changelog pull request remain required before implementation status is earned.

## 0.4.2 recovery evidence — 2026-09-01

- Annotated tag `0.4.2` names protected-main merge `0687551`; the exact push gate run `33460337507` passed before the tag was created.
- Release run `33460657507` anonymously resolved the existing write-once `0.12.0` tag at `sha256:dd901e848c821aca7d55f7b8cf5ee893e1d99a1428b348e32e7ed1045a375319`, then failed while asking `oras manifest fetch --descriptor` for the development/version annotations. OCI registry descriptors carry digest, media type and size; those annotations are members of the manifest content. The run stopped before signing, daemon publication or GitHub release creation.
- The recovery change fetches the anonymous manifest after independently comparing the resolved digest, validates its two annotations, and adds an explicit `workflow_dispatch` recovery for an existing annotated tag. Recovery is accepted only from `refs/heads/main`, re-verifies the immutable tag, its main ancestry, its tagged `Cargo.toml` version and the exact recorded gate success, and never moves the tag.
- The corrected command passed live, credential-free against the public GHCR object. All 10 focused workflow tests and `bash scripts/gate.sh` passed with 2,557 JSON documents across all 12 frozen bundles. Live keyless signing, daemon publication, GitHub release readback and the protected-main changelog pull request remain required before this story can move to `implemented`.
