---
format: aep.planning-md/1
id: story:release-publication-hardening
kind: story
status: active
title: Daemon releases are public, immutable, exact-binary tested and non-overwriting
summary: Harden GHCR visibility, image layout, tag races, summaries and changelog flow.
relations:
- decomposes: epic:release-hardening
revision: 3
---
# Story: Release publication hardening

## Outcome

The exact nonroot image tested by the gate is published once, anonymously retrievable, signed, immutable and truthfully summarized.

## Acceptance

- Fresh volumes satisfy the state-root ownership invariant.
- Release-mode vectors exercise the exact image binary.
- Existing release or image tags are never overwritten.
- Anonymous pull and signature verification precede a draft release publication.
- Failed or skipped steps never produce a success claim.
- Changelog updates travel through a gated PR.
