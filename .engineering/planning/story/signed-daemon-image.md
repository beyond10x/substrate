---
format: aep.planning-md/1
id: story:signed-daemon-image
kind: story
status: draft
title: A tagged main publishes a signed, digest-pinned daemon image
summary: The Dockerfile exists; no workflow builds, pushes or signs it.
owner: substrate
tags:
- ci
- release
relations:
- decomposes: epic:release-hardening
- depends_on: story:ci-runs-the-full-gate
revision: 2
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
