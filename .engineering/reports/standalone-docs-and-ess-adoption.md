# Standalone documentation and initial ESS adoption

## Delivered source

- AEP migration from project/1 to project/5: 119 artifacts, 188 transitions and 143 evidence files.
  `aep plan store migrate git --verify` confirmed equivalent status, revision, title, relations,
  body, transitions and evidence for every artifact. The protocol revision is unchanged; its URL
  now uses public HTTPS. The stable planning scope is `substrate`.
- Repository-owned Rust site builder with 16 explicitly selected pages, retained public guides,
  a new overview/security-boundary diagram, static code highlighting, keyboard navigation and
  exact-commit provenance. Main artifacts use the same immutable project-site publisher as Mantle.
  Repository-side unified docs bundle/check/redirect workflows and their manifest are removed.
- ESS accepted-operation projection: 14 synthesized scenarios executed against production SQLite
  state and persisted events. Full service coverage is not claimed; see `spec/coverage.md`.
- `task check` runs the existing full gate with ESS validation/projection drift and public-docs
  checks added. Immutable wire bundles and production behavior are unchanged.

## Validation observations

ESS 0.50.0 reports `substrate v1 — 2 file(s), valid` and `14 scenario(s) (0 authored), 0 refusal(s)`.
The native ledger run reports 14 passed, 0 skipped, 0 failed. A deliberately corrupted production
mark_dispatch_unknown SQL update was caught by the Unknown-state refusal scenario; the original
SQL was restored. This was an implementation mutation, not a mutation of expected answers.

Browser checks at 1440, 768 and 390 pixels found no document overflow or page errors. The skip link
moves focus to main; public pages carry no scripts. The generated page set passes route/anchor
validation. The build refuses symlink inputs and populated output directories; its public input
allowlist excludes the planning store, internal designs, ADRs and work logs.

The published ESS 0.50.0 Linux archive was downloaded, its exact SHA-256 verified, and its member
layout inspected. CI strips its single top-level directory before adding the binary directory to
PATH. The site workflow is read-only during builds and delegates deployment to the exact pinned
publisher only for a successful main artifact.

## Retained planning findings

Migration does not retroactively invent acceptance evidence. The current validator reports ten
proposed stories without scopes and fourteen historical reviews without recorded outcomes; the
store is valid. Their existing states are unchanged. This change records one adoption story, so no
multi-story decomposition or critic panel was run.

## Publication boundary

This source prepares the standalone Substrate project site. It does not modify the global Website,
Atlas catalog or another repository, and it does not claim that the public site has changed until
the exact commit's deployment and public provenance are verified. An existing global copy of old
Substrate documentation is outside this repository's delivery authority.

## Full gate

`bash scripts/gate.sh` exited 0: workspace tests, formatting, release Clippy, links, ADRs, all-history
Gitleaks, advisory and licence checks, package/MCP boundaries, all sixteen immutable bundle fixed
points, 3,633 classified contract JSON documents and toolchain consistency passed. The portable
workspace run reported 610 passing tests and eight ignored tests; the dedicated delegated and real
project-quota lanes were not run. Five focused documentation boundary tests also passed after the
final renderer hardening. Detailed raw logs are retained in the local worktree recovery evidence.
