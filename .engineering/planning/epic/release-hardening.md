---
format: aep.planning-md/1
id: epic:release-hardening
kind: epic
status: draft
title: Release hardening
summary: A CI gate, a pinned toolchain, a re-observed STATUS.md, and a signed digest-pinned daemon image and contract-bundle artifact; no route, schema or frozen bundle byte changes.
owner: substrate
tags:
- ci
- release
revision: 2
---
# Epic: Release hardening

## Outcome

What the repository says about itself is what a fresh clone, a fresh runner or a consumer can
verify: a pull request cannot show green without `scripts/gate.sh`; local and CI clippy are the
same compiler; `STATUS.md` describes the standalone repository; a tag yields a signed daemon image
and a signed contract-bundle artifact whose digests are in `CHANGELOG.md`.

## Why Now

Substrate went public and cut `0.2.1` on 2026-08-29 (`CHANGELOG.md`). Its working agreement calls
the full gate "the bar for `main`" (`AGENTS.md` § *The gate*), but:

- `.github/workflows/` holds one workflow, `pages.yml`, which builds the website. Nothing runs the
  gate on a push or a pull request.
- No `rust-toolchain.toml` exists; `AGENTS.md:116-119` documents the drift ("a newer clippy can
  fail a commit that passed locally") and asks for `rustup update` instead of removing the cause.
- `Dockerfile` builds a distroless `substrate-daemon`; nothing publishes, signs or digest-pins it.
  `README.md` § *Status*: "stable publication: **not done**".
- Agent consumes the 0.4.0 bundle as a hand-copied tree (`docs/plan/04-direct-byte-plane.md` §
  *Slice C*); design 07 § *Bundle* already fixes the OCI shape and nothing implements it.
- Two `AGENTS.md` facts contradict the scripts they describe (`AGENTS.md:112` vs
  `scripts/gate.sh:20-23`; `AGENTS.md:172` vs `scripts/bot-token.sh:8`), and `STATUS.md` is
  observed 2026-08-17, before extraction, still naming the monorepo as the source.

## Scope

Six stories, worked in this order because each makes the next checkable:

1. `story:agents-md-matches-the-scripts` — docs only.
2. `story:ci-runs-the-full-gate` — `gate.yml`, SHA-pinned actions, `contents: read`.
3. `story:pinned-rust-toolchain` — `rust-toolchain.toml` = `1.97` = `Cargo.toml` `rust-version` =
   `Dockerfile` builder tag; one commit per bump.
4. `story:status-md-re-observed` — from fresh gate output.
5. `story:signed-daemon-image` — `release.yml` on a bare-version tag, GHCR, cosign keyless.
6. `story:contract-bundle-oci-artifact` — deterministic OCI layout, published beside the image.

## Out of Scope

Any route, schema, wire identifier, capability or frozen bundle byte. The development bundles stay
development bundles; a stable-contract decision is its own ADR under atlas ADR 0019.
Bit-for-bit reproducible binaries: digests are recorded, reproduction is a later milestone.

## Risks

- `packages: write` and `id-token: write` widen a workflow's authority; scope them to the release
  job only.
- The delegated lane needs bubblewrap and cgroup delegation and cannot run on a hosted GitHub
  runner; either a self-hosted runner joins, or CI asserts the portable lane and the delegated
  lane stays a local pre-release step. The CI story records which.
- Keyless signing depends on Sigstore availability; a release fails closed, never publishes
  unsigned.

## Done When

From a fresh clone: `AGENTS.md` matches the scripts; a PR with a formatting error goes red; clippy
runs on the pinned toolchain; `STATUS.md` names the standalone repository and the current tag; a
tag `0.x.y` yields a signed image and a signed bundle artifact whose digests are in `CHANGELOG.md`.
