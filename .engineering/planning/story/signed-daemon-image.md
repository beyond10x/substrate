---
format: aep.planning-md/1
id: story:signed-daemon-image
kind: story
status: active
title: A tagged main publishes a signed, digest-pinned daemon image
summary: The Dockerfile exists; no workflow builds, pushes or signs it.
owner: substrate
tags:
- ci
- release
relations:
- decomposes: epic:release-hardening
- depends_on: story:ci-runs-the-full-gate
revision: 5
---
# Story: A tagged main publishes a signed, digest-pinned daemon image

## Outcome

An operator can `docker pull ghcr.io/beyond10x/b10x-substrate-daemon@sha256:…` for a tag, verify
its signature, and know it was built by the bot from that tagged commit.

## Context

`Dockerfile` builds a distroless `substrate-daemon` with an `org.opencontainers.image.revision`
label; no workflow builds, pushes or signs it, and `README.md` § *Status* records "stable
publication: **not done**". `README.md:31` fixes the package prefix `b10x-substrate-*`.
`AGENTS.md` § *Bot identity* requires automated pushes to use the GitHub App, and § *Releases*
fixes the tag form: the bare version, annotated, at a fully gated `main` commit.

## Acceptance

An annotated bare-version tag on `main` produces a keyless-signed image at
`ghcr.io/beyond10x/b10x-substrate-daemon:<version>` whose digest is written to the GitHub release
and under the version heading in `CHANGELOG.md`, and a pre-release tag such as `0.2.2-rc.0`
produces nothing.

Evidence that satisfies it:

- `.github/workflows/release.yml` triggers only on the bare-version form and refuses to build
  unless `gate.yml` is green for that commit;
- `SOURCE_SHA` = the tag's commit; `cosign verify` with the workflow identity succeeds; a signing
  failure fails the job before anything is announced;
- release notes state that the wire contract bundles remain development bundles;
- `permissions` are `contents: read` at workflow level and `packages: write`, `id-token: write` on
  the release job only; every action SHA-pinned;
- the pre-release-tag run URL recorded here showing no publish.

## Out of Scope

Bit-for-bit reproducible binaries: the sha256 is recorded, reproduction is a later milestone. The
image's `VOLUME` and `EXPOSE 8080` describe the development-only TCP posture; release notes must
not describe the image as a hosted posture (design 06 § 1).

## Progress — 2026-08-30, the workflow exists and has never run

Merged as `fced5fb` (PR #22). `.github/workflows/release.yml` builds the `Dockerfile`, publishes
`ghcr.io/beyond10x/b10x-substrate-daemon:<version>`, keyless-signs and digest-pins it.

**No image is published.** The workflow existing and an image existing are different claims;
`README.md`, `STATUS.md` and `AGENTS.md` § *Releases* each keep them apart.

Acceptance, item by item:

| evidence | state |
|---|---|
| triggers only on the bare-version form | **met** — coarse ref glob plus an anchored regex in preflight; `0.2.2-rc.0`, `v0.2.2`, `0.2` and `0.2.2.1` all refuse |
| refuses to build unless `gate.yml` is green for that commit | **met** — preflight reads that workflow's own recorded conclusion for the tagged SHA rather than re-running a lookalike |
| `SOURCE_SHA` = the tag's commit | **met** — `git rev-parse refs/tags/<tag>^{commit}`, cross-checked against `github.sha` |
| a signing failure fails the job before anything is announced | **met** — build → push → sign → `cosign verify` → *then* token, notes, release, changelog |
| notes state the bundles remain development bundles | **met** — § *What this release does not claim*, citing atlas ADR 0019 |
| `permissions` scoped; every action SHA-pinned | **met** — `packages: write`/`id-token: write` on the release job alone; preflight holds `actions: read` because no smaller scope reads another workflow's conclusion for a SHA |
| `cosign verify` with the workflow identity succeeds | **open** — needs a real tag push |
| a pre-release-tag run URL showing no publish | **open** — proving the negative needs `0.2.2-rc.0` pushed |

All four action pins were resolved against their upstream repositories rather than taken on report:
`actions/checkout@3d3c42e5` is `refs/tags/v7` and `v7.0.1`; `sigstore/cosign-installer@6f9f1778` is
`refs/tags/v4.1.2`; `actions/create-github-app-token@bcd2ba49` is `refs/tags/v3` and `v3.2.0`.

## Blocked on the operator

1. Repository secrets `B10X_BOT_APP_ID` and `B10X_BOT_PRIVATE_KEY`, and a b10x-bot installation
   with `contents: write`. Absent them the job fails **after** the image is pushed and signed and
   **before** anything is announced — a signed image with no release.
2. A real tag push, which is the only thing that can close the two open evidence items. Publishing
   is outward-facing and was not authorised, so no tag was created.

## Related: the credential scan went red, and why it is green again

Committing bundle `0.7.0` for `story:ledger-rows-carry-the-declared-grant` turned
`scripts/check-secrets.sh` red — 32 findings, every one the `jwt` rule on a delegated-context
conformance vector. Those vectors are JWTs by definition (ADR 0011 fixes the document as a compact
JWS) and carry no credential. `.gitleaks.toml` keeps the default rule set and allows the `jwt` rule
for the delegated-context vector paths alone. Proven narrow rather than asserted: the same token is
allowed in `contracts/substrate-wire/0.7.0/vectors/http/delegated-context-records-grant.json` and
still caught in `crates/substrate-daemon/src/`. `bash scripts/check-secrets.sh` → `no leaks found`,
76 commits.
