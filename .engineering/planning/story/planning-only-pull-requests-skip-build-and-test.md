---
format: aep.planning-md/3
id: story:planning-only-pull-requests-skip-build-and-test
kind: story
status: implemented
title: A planning-only pull request passes its required checks without build or test
summary: One path list; the required job classifies the changed files and exits 0 for planning and prose
tags:
- defect
- gate
revision: 4
transitions:
- {from: "draft", to: "proposed", at: "2026-11-21T21:48:10Z", actor: "human:timo", revision: 2}
- {from: "proposed", to: "active", at: "2026-11-21T21:48:10Z", actor: "human:timo", revision: 3}
- {from: "active", to: "implemented", at: "2026-11-25T19:09:38Z", actor: "agent:substrate", revision: 4, decided_on: {"recorded":{"test_result":1}}}
---
# Story: a planning-only pull request passes its required checks without build or test

## Outcome

A pull request whose changed files are all planning or prose files passes the required checks
without compiling or testing anything: the required job runs, finds only such paths and exits 0.

## Where the requirement comes from

The operator's rule of 2026-11-21 for every repository of the collection, section
*Planning-only pull requests skip the tests* of the collection folder's `AGENTS.md`, relayed by
the operator session `host-operator` with the order to file this story. The rule says it is
built ahead of the next feature, like a test-budget defect. Stakeholder: the operator.

## Measured

On 2026-11-21 this repository merged 7 pull requests (238, 240, 241, 242, 244, 245, 246). Four
of them (238, 240, 242, 246) changed only files under `.engineering/` or design documents, and
each waited for a full round of the hosted checks. In the round of pull request 245 the job
*Gate: tests and Clippy* took 15 min 33 s. The four rounds' own times were not read.

## What changes

1. One path list in the repository, read by the workflows: `.engineering/` and `*.md` files
   outside the built site (`website/`). A changed file outside the list makes the pull request a
   code change.
2. The jobs behind the required contexts (`Full gate` of `gate.yml`, and what the shared source
   gates require) classify the changed files first. For a planning-only change the build, test
   and lane jobs do not run and the required job exits 0, saying which paths it found.
3. A change under `website/` still runs `cargo xtask check-docs` and the documentation build; it
   skips the code's build and tests when nothing else changed.
4. The checks that read prose still run on a planning-only change, since they need no build of
   the workspace beyond `xtask`: decide and state which (`check-links`, `check-adrs`,
   `check-secrets`, the planning store's validation). A broken link in a design document must
   not reach `main` through this path.
5. A test executes the classifying step offline against throwaway repositories, as
   `xtask/tests/release_workflow.rs` does for the release steps, with a case for both sides: a
   change of only listed paths, and a change that adds one file outside the list.
6. `AGENTS.md` § *The gate* states the path list and what still runs.

## Not changed

- The rulesets (`main-requires-the-full-gate`, the shared required gates, "branch must be up to
  date") stay as they are; they are the operator's.
- `release.yml` still requires `gate.yml`'s `success` for the tagged commit from a `push` run on
  `main`. A planning-only merge commit must therefore still get a full gate run on `main`, or
  the release preflight must be shown to accept it; the story states which and tests it. Not
  decided here.
- The release lanes on `main`.

## Risk to state in the design of the step

A pull request that changes a listed path and renames or deletes a code file must classify as
code. The classification reads every changed path of the pull request against its merge base,
both sides of a rename.

## Acceptance

A pull request that changes only `.engineering/` files has its required contexts green with no
`cargo build` or `cargo test` in its logs; the same pull request with one `.rs` file added runs
the full gate; the offline test holds both cases and a rename out of a listed path.
