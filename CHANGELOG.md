# Changelog

All notable changes to Substrate are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- **A workspace root may be a directory the operator already owns.** A root name no longer has to
  carry the `ws_` prefix, only to be a single path component; containment was always `openat2`
  beneath the pinned root descriptor with symlinks refused, never the prefix. A harness can now run
  against your actual checkout — `harness`, `engineering-protocols` — instead of only a `ws_`-named
  scratch copy. Workspaces the server creates are still minted as `ws_…`.

## [0.2.1] — 2026-08-29

Documentation and distribution release. This release changes no runtime behavior, route, schema,
wire identifier, capability, or contract-bundle byte.

### Added

- A self-contained public Docusaurus site with a project-specific landing page, eight reader-facing
  documentation pages, responsive themes, a Substrate mark, and strict broken-link and
  broken-anchor gates.
- A GitHub Pages workflow that installs from the npm lockfile, type-checks, builds, uploads only
  `website/build`, and deploys the resulting artifact from `main`.
- A public-website working agreement that requires the Atlas `website-docs` skill and keeps
  internal designs, ADRs, plans, reviews, work logs, contributor status material, and source links
  out of the reader-facing site.

### Changed

- The repository is public under Atlas ADR 0003 after a full-history and working-tree credential
  scan found no leaks. Public visibility does not change the proprietary licence and does not make
  the development contract bundles stable.
- The workspace crate version is `0.2.1`.

## [0.2.0] — 2026-08-24

First release from the standalone repository. Substrate was extracted from the b10x monorepo
at `e01ea676` with full history on 2026-08-23; everything before the extraction is recorded in the
monorepo's own ledgers, and the version continues from the manifest it arrived with.

### Added

- **Declared host roots are mounted read-only** (ADR 0010). A root the operator names is bound
  `--ro-bind` into the confined tree, so a run can read a declared host tree and can never write
  through it.

### Changed

- The shared cargo cache mounts are `sharing=locked`, so parallel confined builds cannot corrupt
  one cache.
- This repository is the canonical Substrate home, with its own gate (`bash scripts/gate.sh`); the
  surface speaks as b10x and a fence keeps it that way, and links that escape the repository are
  pinned to the extraction baseline.
