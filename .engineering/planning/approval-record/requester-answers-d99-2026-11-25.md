---
format: aep.planning-md/3
id: approval-record:requester-answers-d99-2026-11-25
kind: approval-record
status: active
title: The requester's answer on four finished stories and one dropped story (2026-11-25)
summary: 'D99 = A: four stories move to implemented on the delegated-lane runs of 0.27.0 and 0.28.0; the empty-list story is dropped'
relations:
- decides: story:run-observation-names-the-limit-that-ended-it
- decides: story:audit-records-run-end-and-limit
- decides: story:process-count-limit-hit-is-observed
- decides: story:exec-start-carries-executable-lists
- decides: story:explicitly-empty-exec-subpaths-means-the-program-only
revision: 3
transitions:
- {from: "draft", to: "active", at: "2026-11-25T15:33:45Z", actor: "agent:substrate", revision: 2}
---
# The requester's answer on four finished stories and one dropped story (2026-11-25)

## Who decided

The requester of `epic:runs-are-governed-and-limit-hits-are-events`, the stakeholder the stories
of that epic name, under the rule that the stakeholder of a request decides for the work it
requested (`AGENTS.md` § *Planning artifacts*).

## The question

Asked on 2026-11-25 as decision D99, after release 0.28.0, with option A as the default:

- A: move four stories to `implemented`: `run-observation-names-the-limit-that-ended-it`,
  `audit-records-run-end-and-limit`, `process-count-limit-hit-is-observed` and
  `exec-start-carries-executable-lists`.
- B: move the three that stand at `active` and leave `exec-start-carries-executable-lists` until
  each of its remaining checks is read.
- C: move none.

The question also named `explicitly-empty-exec-subpaths-means-the-program-only`, which waited
for the requester's word to start or to drop it.

## The answer

> D99 = A, move all four, each move resting on a test_result record that names the
> delegated-lane run of 0.28.0 (and 0.27.0 for the three active ones). Keep the oci memory
> flake's draft open beside run-observation-names-the-limit-that-ended-it.
> explicitly-empty-exec-subpaths-means-the-program-only: drop. Runbook reaches substrate only
> through Loom's confinement and no Runbook story or consumer gap needs an empty list to mean
> "program only"; a client that wants the strict rule lists the program. If a gap needs it later,
> Runbook files a new ask.

## What was read before the question

On `main` at `224e1e5ffd`, by a read-only audit that built and ran nothing:

- `run-observation-names-the-limit-that-ended-it`: the cases at
  `crates/substrate-host/tests/ess_confinement_linux.rs` for a memory, CPU-time and wall-time
  limit and for no limit, each required by `scripts/delegated-lane.sh` by its `executed` line.
- `audit-records-run-end-and-limit`: the case `ESS confinement linux run ended memory`, required
  the same way.
- `process-count-limit-hit-is-observed`: the three cases `ESS confinement linux process limit
  hits over`, `under` and `running`, required the same way.
- `exec-start-carries-executable-lists`: the five cases `runtime exec lists …` of
  `crates/substrate-daemon/tests/runtime_vectors.rs`, required the same way, and the
  `CHANGELOG.md` section of 0.28.0. The audit read about 15 of the story's 25 checks; the
  result-reserve and bundle-module cases were not read again.

The log of the hosted confinement lane for release 0.28.0 (job `Confinement lane` of
https://github.com/beyond10x/substrate/actions/runs/45620955579, commit `e7faf3b1d6`, 222
delegated cases) holds every one of those `executed` lines. The lane of release 0.27.0 is
https://github.com/beyond10x/substrate/actions/runs/45483880771 (commit `c5f0b8716d`, 201
delegated cases).

## What follows

- Each of the four stories gets a `test_result` record naming the lane run of 0.28.0, and the
  three that stood at `active` a second one naming the lane run of 0.27.0, before its move.
- `story:an-oci-run-killed-at-its-memory-bound-names-the-limit-on-a-hosted-runner` stays a draft.
- `explicitly-empty-exec-subpaths-means-the-program-only` is archived. A later need is a new
  request.
