# Changelog

All notable changes to Substrate are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
