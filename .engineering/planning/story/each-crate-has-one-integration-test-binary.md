---
format: aep.planning-md/3
id: story:each-crate-has-one-integration-test-binary
kind: story
status: draft
title: Each crate has one integration-test binary
summary: The gate's test build links one test binary per crate; hosted build time is 743 s with 173 binaries
tags:
- defect
revision: 1
---
# Story: each crate has one integration-test binary

## Outcome

The gate's test build links one integration-test binary per crate (`tests/main.rs` with
modules), as the collection's rule "Tests stay fast" requires, and the hosted test build is
measured before and after.

## Where the requirement comes from

The collection's rule, `AGENTS.md` of the folder above this repository, section "Tests stay
fast": "Test binaries are few: one integration-test binary per crate (`tests/main.rs` with
modules)". Left open by `story:gate-test-step-ends-within-two-minutes`, which bounded the test
run and not the build. Stakeholder: the operator.

## Measured

- Hosted, pull request 250 at `a36c5b261`: `gate-time: test build: 743 s`, `test run: 98 s`,
  2963 tests.
- Counted on a workstation during that story: 173 test binaries, 1401 MB; the host crate has 64
  integration-test files, the daemon 35.

## What changes

1. Per crate, in the order of their file counts (host, daemon, xtask, CLI, the rest): the files
   under `tests/` become modules of one `tests/main.rs`. Shared helpers under `tests/support/`
   stay one module.
2. Every place that names a test binary follows: `--test <name>` in `scripts/gate.sh`,
   `scripts/delegated-lane.sh`, the workflows, `.config/nextest.toml` filters and test groups,
   `AGENTS.md`, the release workflow's count greps, the lanes' `require_staged` lines.
3. A case that needs a process of its own (environment variables it sets, a signal handler)
   is named and keeps its binary, with the reason in `AGENTS.md`.
4. The test set before and after is compared by name, as the budget story's adversary did; no
   test is lost.

## Design

No contract changes. One crate per pull request, so that a red lane points at one move.

## Acceptance

- After each crate: `cargo nextest list` shows the same test names, apart from the module prefix.
- At the end: one `tests/main.rs` per crate apart from the named exceptions; the hosted
  `gate-time: test build:` is reported beside 743 s; `test run:` stays within 120 s.

## Not known

- How much of the 743 s is linking and how much is compiling the crates themselves. The first
  crate's move measures it.
