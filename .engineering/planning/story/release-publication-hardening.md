---
format: aep.planning-md/3
id: story:release-publication-hardening
kind: story
status: implemented
title: Daemon releases are public, immutable, exact-binary tested and non-overwriting
summary: Harden GHCR visibility, image layout, tag races, summaries and changelog flow.
relations:
- decomposes: epic:release-hardening
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-08-31T08:37:44Z", actor: "human:timo", revision: 2, imported: true}
- {from: "proposed", to: "active", at: "2026-08-31T08:37:44Z", actor: "human:timo", revision: 3, imported: true}
- {from: "active", to: "implemented", at: "2026-08-31T22:49:58Z", actor: "human:timo", revision: 4, decided_on: {"recorded":{"test_result":1}}, imported: true}
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
