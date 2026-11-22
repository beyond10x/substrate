---
format: aep.planning-md/3
id: story:local-gate-holds-what-the-hosted-runner-differs-in
kind: story
status: active
title: The local gate holds what the hosted runner differs in
summary: Runner images named by version, the 26.04 tool layout held locally, a wait script that exits non-zero on red
tags:
- gate
revision: 3
transitions:
- {from: "draft", to: "proposed", at: "2026-11-21T22:48:54Z", actor: "human:timo", revision: 2, executor: "agent:host-operator"}
- {from: "proposed", to: "active", at: "2026-11-21T22:48:54Z", actor: "human:timo", revision: 3, executor: "agent:host-operator"}
---
# Story: the local gate holds what the hosted runner differs in

## Outcome

What the hosted runner does differently from the workstation is written in the repository and
used by the local gate, the runner image is named by version, and the script that waits for a
pull request's checks exits non-zero when a check ends red.

## Where the requirement comes from

The operator's rule of 2026-11-21 for every repository of the collection, section *The local
gate equals the forge* of the collection folder's `AGENTS.md`. The operator session asked this
repository to check the rule against its hosted-only failure of the same day. Stakeholder: the
operator.

## The check against the failure of 2026-11-21

Pull request 241 was green locally and red on the hosted gate: a lane case ran `ln` from a
listed `/usr/bin`, and on the runner (`ubuntu-latest`, Ubuntu 26.04 that day) `/usr/bin/ln` is a
symbolic link into `/usr/lib/cargo/bin/coreutils`, which the helper correctly does not admit.

Held today:

- One pinned toolchain read by both: `rust-toolchain.toml` (channel `1.97`), which `gate.yml`
  reads; `cargo xtask check-toolchain` holds the three pins together.
- The fix included a case that catches the same failure locally:
  `limit_a_link_in_a_listed_directory_that_points_outside_it_is_not_admitted` in the helper's
  `gate` root stages the layout on any host.
- The difference is written down: `AGENTS.md` § *The gate* names the 26.04 layout.

Not held:

- `gate.yml`, `chart-lane.yml`, `namespace-lane.yml`, `pages.yml` and `release.yml` run on
  `ubuntu-latest`, an image that moves without a commit here. `release-lanes.yml` pins
  `ubuntu-24.04`.
- `AGENTS.md` says the host crate's lane cases list their tools' directories through
  `base_directories()` and "would fail on a 26.04 runner". Nothing local runs them against that
  layout, so the next move of an image can be red on the forge after a green local gate.
- The channel `1.97` names no patch release; the workstation and the runner can hold different
  ones. Not observed to differ.
- A hosted runner grants a user namespace no mount, so the staged-mount cases run on a
  workstation only. This is the reverse gap and is stated in the changelog; it is not closed by
  this story.
- No script of the repository waits for a pull request's checks. Sessions use `gh pr checks
  --watch`, whose exit status is non-zero on a red check, or scripts of their own that only
  print.

## What changes

1. Every workflow names its runner image by version, and `AGENTS.md` says which.
2. The host crate's lane cases resolve each tool to the directory its file lies in, or the lane
   stages a layout where the tools of a listed directory are links out of it, so the 26.04
   layout is held on a workstation.
3. `rust-toolchain.toml` pins a full version, or the story states why the channel is kept.
4. One script of the repository waits for a pull request's or a commit's checks and exits
   non-zero when one ends red, with a test of its exit status on recorded check lists.

## Acceptance

No workflow runs on `ubuntu-latest`; the host lane cases pass on a workstation with the staged
26.04 layout; the wait script exits non-zero for a recorded red check list and zero for a green
one.
