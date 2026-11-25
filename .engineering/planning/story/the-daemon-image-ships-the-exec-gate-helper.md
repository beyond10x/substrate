---
format: aep.planning-md/3
id: story:the-daemon-image-ships-the-exec-gate-helper
kind: story
status: draft
title: The daemon image ships the executable-list helper
relations:
- decomposes: epic:runs-are-governed-and-limit-hits-are-events
- depends_on: story:exec-start-carries-executable-lists
- informed_by: story:daemon-image-serves-exec-or-says-it-cannot
revision: 1
---
# Story: The daemon image ships the executable-list helper

## Outcome

An operator who installs the released daemon image or the chart serves the executable lists
without building anything: the image holds the static helper `substrate-exec-gate`, and one
chart value starts the daemon with `--exec-gate` naming it.

## Where the requirement comes from

- Design 37 § 7 and question Q5 of its § 15: the release image and the chart are a story of
  their own, so that `story:exec-start-carries-executable-lists` changes no release artifact.
  Answered A by the operator's session on default
  (`approval-record:operators-session-answers-the-questions-of-design-37-2026-11-25`). The
  stakeholder of the wire story, the session `sdc`, may reopen it.
- The relayed requirement of 2026-11-21 (`epic:runs-are-governed-and-limit-hits-are-events`):
  runs move remote and keep their executable lists.

## Why

- After the wire story a daemon serves the lists only when it is started with `--exec-gate`
  naming a statically linked helper. Neither `Dockerfile` nor `deploy/helm/substrate` names the
  helper at `main` `7641b34c1`, so an operator builds it for the musl target and mounts it.
- The driver refuses a helper with a second hard link or a loader
  (`config.exec-gate-invalid`), so where the file lies in the image and how it got there is
  part of what the image promises.

## What changes

This is a draft filed with the wire story's plan. Three names are proposals and are fixed when
this story is planned, before code: the stage, the path and the value key.

1. `Dockerfile`. Proposal: the `executor-builder` stage, which already builds for
   `x86_64-unknown-linux-musl`, builds `b10x-substrate-exec-gate` in its one `cargo` call, and
   the `daemon` target copies the one file to `/usr/local/libexec/substrate-exec-gate`. The
   cases of `xtask/tests/release_workflow.rs` that count the `cargo build` calls of each stage
   and of the whole file move only if another shape is chosen.
2. `deploy/helm/substrate`. Proposal: `execution.executableLists.enabled`, `false` by default,
   adds `--exec-gate /usr/local/libexec/substrate-exec-gate` to the daemon's arguments.
   Whether the value is refused without `execution.enabled` is this story's own question, not
   design 37's: the flag configures the host driver's exec path, and the chart's other
   execution values are refused that way. Decided in the plan.
3. `cargo xtask check-image-startup` (`xtask/src/image_startup.rs`), which `release.yml` runs
   in two steps on the built daemon image, also refuses an image whose helper has a
   `PT_INTERP`, has more than one hard link, or does not answer `--version`. Whether the verb
   can also start the image's daemon with `--exec-gate` is not read yet (inferred); the plan
   settles it.
4. `website/docs/guides/deployment.md` names the value and the path, and links to the sentence
   on the served volumes, which the wire story writes and this story leaves as it is.
   `README.md` (the paragraph that says which files the published daemon image carries),
   `CHANGELOG.md` and `AGENTS.md` (§ *Releases*, the image's content) move with it.

## Acceptance

`cargo xtask chart-lane static` renders the chart in both states of the value: off, the
daemon's arguments are those of `main` before this story, compared with a rendering the case
holds as text; on, they hold the one flag more and nothing else differs.

Checks:

- **The image**, observed by `check-image-startup` on the built image, which `release.yml`
  runs before any tag moves and which runs on a workstation with a Docker engine: the helper at
  the fixed path has no `PT_INTERP`, has one hard link and answers `--version`; a daemon started
  from the image with `--exec-gate` naming it opens its host driver.
- **The lists are served from the image**, one workstation run recorded as evidence in the
  planning store, not a test of the gate: the chart installed with the value on, on a kind
  cluster built as `cargo xtask chart-lane cluster --node-profiles` builds its own, and the
  daemon's capability snapshot read through the pod's socket. The lane installs one fixed
  chart today (`xtask/src/chart_cluster.rs`) and has no option for a value: the plan picks
  between an option of the verb, which puts that file in scope, and a `helm upgrade --set`
  by hand on the lane's cluster, and names the command that reads the snapshot. Both facts
  present and no `withheld` is the pass. A snapshot with `withheld` is not a pass: the run records the cause
  and the file-system type the daemon's standard-error line names, and the story stays open
  until the guide says on which node volumes the chart serves the lists. No entry joins
  `SCENARIOS`, so the count of the `Chart-lane:` record does not move.
- **The value off** changes no running daemon: the hosted `chart-lane.yml` scenarios pass
  unchanged.
- The offline cases of `xtask/tests/release_workflow.rs` hold the new refusals of item 3 as
  text of the workflow.
- No test of the gate's test step builds an image or starts a cluster. Every new test is timed
  before it is added; none runs over 60 s, and the test step stays within 120 s.

## Boundaries

- No change of the wire and no new bundle.
- The MCP image and the executor image do not get the helper.
- No default-on: a chart upgrade changes no running daemon's facts.
- No new chart-lane scenario and no change of a release record's count.

## Order

After `story:exec-start-carries-executable-lists`, which adds the flag. Whether it rides the
release that carries the wire story or a later one is the operator's.

## Scope

- `Dockerfile`. Cited: the stages `builder`, `executor-builder`, `daemon`.
- `deploy/helm/substrate`. Cited: `values.yaml`, `execution.enabled`.
- `xtask/src/image_startup.rs`. Cited: the verb `check-image-startup`, its `DAEMON` path
  constant.
- `xtask/src/chart.rs` (the static lane), `xtask/tests/release_workflow.rs`. Inferred.
  `xtask/src/chart_cluster.rs` only if the plan gives the lane an option for the value.
- `.github/workflows/release.yml`. Cited: the two `check-image-startup` steps.
- `README.md`. Cited: "The published daemon image carries the two files item 3 names".
- `website/docs/guides/deployment.md`, `CHANGELOG.md`, `AGENTS.md`. Inferred.
