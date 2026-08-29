---
format: aep.planning-md/1
id: story:ci-runs-the-full-gate
kind: story
status: draft
title: CI runs the full gate on every push and pull request
summary: Only pages.yml exists under .github/workflows/; nothing runs scripts/gate.sh on a push or PR.
owner: substrate
tags:
- ci
relations:
- decomposes: epic:release-hardening
revision: 2
---
# Story: CI runs the full gate on every push and pull request

## Outcome

A pull request against `main` cannot show green unless `bash scripts/gate.sh` passed on a fresh
runner.

## Context

`.github/workflows/` holds one workflow, `pages.yml`, which builds the website. The gate
`AGENTS.md` § *The gate* calls "the bar for `main`" runs only on whoever remembers to run it. The
repository is public (atlas ADR 0003; `CHANGELOG.md` 0.2.1), so pull requests will arrive from
people who have not read `AGENTS.md`.

The Python checkers are stdlib-only and shell out to the built `substrate-contract-check` crate
(`scripts/contract_json_gate.py:185`; `jsonschema = "=0.49.9"` in `Cargo.toml`), so the workflow
installs no pip packages.

## Acceptance

`.github/workflows/gate.yml` runs `bash scripts/gate.sh` on `push` to `main` and on `pull_request`,
exits with the gate's own status, and a throwaway branch with one `cargo fmt` violation produces a
red check that the fix turns green.

Evidence that satisfies it:

- every action SHA-pinned with a version comment; `permissions: contents: read`; `timeout-minutes`
  — the `pages.yml` conventions;
- the toolchain is `1.97`, pinned explicitly until `story:pinned-rust-toolchain` makes
  `rust-toolchain.toml` the source;
- the job log names which lane ran; on a hosted runner without bubblewrap/cgroup delegation the
  delegated lane is reported **absent**, never passed (invariant 3);
- the two run URLs (red, then green) recorded in this story;
- `bash scripts/gate.sh` exits 0 locally.

## Open Questions

Whether the delegated lane gets a self-hosted runner or stays a local pre-release step. Decides:
operator. Default if nobody answers: **stays local**; CI asserts the portable lane and prints the
delegated lane as absent.

Branch protection (require the `gate` check before merge) is a GitHub setting, not a file; it is a
one-line "to try" for the operator after the first green run.
