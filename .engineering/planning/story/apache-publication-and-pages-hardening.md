---
format: aep.planning-md/1
id: story:apache-publication-and-pages-hardening
kind: story
status: draft
title: Substrate is Apache-2.0 with a hardened public distribution
summary: Relicense beyond10x-owned history, expose a safe public support surface, and gate the source, site and image for public use.
owner: substrate
tags:
- release
- security
- website
relations:
- decomposes: epic:release-hardening
revision: 1
---
# Story: Substrate is Apache-2.0 with a hardened public distribution

## Outcome

A reader can use Substrate under Apache-2.0, find its intentionally public source and private
security-reporting path, build trustworthy public documentation, and pull a signed daemon image
without authentication.

## Context

Atlas ADR 0010 grants Apache-2.0 across all beyond10x-owned Substrate history without rewriting any
frozen contract bytes. The repository and Pages site are already public, but GitHub secret scanning,
push protection, private vulnerability reporting and Dependabot security updates are disabled; the
GHCR image is not anonymously readable; and the website build has known transitive advisories.

## Acceptance

1. Root licence and Cargo metadata say Apache-2.0; third-party material keeps its own licence and the
   daemon image carries deterministic notices.
2. The full reachable history and every worktree intended to land pass the pinned secret scan; the
   synthetic Axum WebSocket fixture needs no broader allowlist.
3. GitHub secret protections, private reporting, dependency updates and action SHA pinning are
   enabled, and anonymous clients can pull the signed daemon image.
4. The public site builds from locked, audit-clean dependencies, documents only implemented behavior,
   and links only the deliberately public repository, licence and security-reporting destinations.
5. The complete repository gate, delegated lane, exact image tests and Pages deployment pass.

## Out of Scope

This does not make a development wire bundle stable, publish crates to crates.io, promise a support
SLA, or publish internal designs, ADRs, plans, reviews, work logs or contributor status material.

## Open Questions

None. The operator selected Apache-2.0 for the complete beyond10x-owned history and selected staged
landing with explicit source and security links.
