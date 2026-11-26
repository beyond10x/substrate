---
format: aep.planning-md/3
id: approval-record:wave-2026-11-26-substrate
kind: approval-record
status: approved
title: 'Wave 2026-11-26: run usage and the local gate, with two closing moves'
relations:
- decides: story:run-observation-carries-usage
- decides: story:local-gate-holds-what-the-hosted-runner-differs-in
- decides: story:test-fifos-made-in-process
- decides: story:an-oci-run-killed-at-its-memory-bound-names-the-limit-on-a-hosted-runner
revision: 2
transitions:
- {from: "draft", to: "approved", at: "2026-11-26T04:33:05Z", actor: "human:operator", revision: 2}
---
# Wave 2026-11-26: run usage and the local gate, with two closing moves

Coordinator: the session that works this repository. Skill `aep:implementing` 0.26.0, wave mode.

## The approval

The repository owner's order of 2026-11-26: plan the next wave with several stories and the open
gap closures together, on one integration branch, delivered as one pull request, without asking
again for each step. This record is that wave's page and its approval.

## The plan it is one wave of

Derived on 2026-11-26 from the accepted stories with
`aep plan artifact waves --kind story --status active` (aep 0.80.0), after the typed scope of the
two units below was recorded:

1. `story:local-gate-holds-what-the-hosted-runner-differs-in`
2. `story:run-observation-carries-usage`
3. `story:exec-file-changes-are-recorded` (depends on 2)
4. `story:exec-file-change-record-on-the-wire` (depends on 3 and on the draft
   `story:exec-observation-reports-process-count-hits`, which is not accepted)
5. `story:aperture-reports-dial-outcome` (depends on 4)
6. `story:resolver-aperture-serves-dns` (depends on 5)

The verb puts 1 and 2 in separate waves on one inferred shared file (`scripts/delegated-lane.sh`).
This wave takes both and merges them into the integration branch one after the other, so the
shared file is resolved at the second merge. Stories 3 to 6 stay in order behind it. Story 4 cannot
start until its draft dependency is accepted.

Not planned: `story:kubernetes-namespace-workspace-and-exec-slice`, which records no scope and is
unchanged since 2026-10-29.

## Units

| unit | story | stakeholder | first step (invariant 8) |
|---|---|---|---|
| U1 | `story:run-observation-carries-usage` | `sdc` | a usage section in `docs/design/36-limit-hits.md` (D29 and on) |
| U3 | `story:local-gate-holds-what-the-hosted-runner-differs-in` | the repository owner | none: no contract or capability moves |

U1's design answers, the story-scoper's recommendation for each, taken as the default because the
stakeholder had no running session on 2026-11-26: one `#[non_exhaustive]` `RunUsage` with four
`Option<u64>` members; a counter that cannot be read leaves only that member absent;
`wall_time_us`, `cpu_time_us`, `memory_peak_bytes` and `processes_peak`, as the wire's
`ResourceUsage` names them; wall time stops at the leader's exit; an exec that asked for
measurements reuses its final sample; a run that ends `Unknown` carries what was read before the
cgroup went, and D22's drop record carries none; the specification keeps D25's `UNMAPPED:` block,
extended. Each is a decision of the design section and can be changed there before the pull request
merges.

U3's choices: every Linux workflow runs on `ubuntu-26.04`, the image `ubuntu-latest` is on
2026-11-26, and `release-lanes.yml` stays on `ubuntu-24.04`; the lane cases resolve each tool to
the directory its file lies in; `rust-toolchain.toml` names a patch release only if
`cargo xtask check-toolchain` and the `Dockerfile`'s pinned image can name the same one, otherwise
the story states why the channel stays; the wait script is POSIX sh.

## Closing moves in the same pull request

- `story:test-fifos-made-in-process` to `implemented`: no `Command::new("mkfifo")` remains in the
  workspace on `main` at `4301004012` (commit `fd21aacecf`, pull request 222), and the gate run
  45644944907 on `main` at `224e1e5ffd` concluded `success`.
- `story:an-oci-run-killed-at-its-memory-bound-names-the-limit-on-a-hosted-runner` to
  `implemented`, after pull request 307 merges and `main`'s gate is green on it: the repeat
  measurement of run 45750181878 counted 410 runs, 286 `Memory`, 124 stated, 0 neither.

## Branches and trees

- Integration branch: `wave/2026-11-26`, cut from `main` after pull request 307 merges.
- One managed worktree per unit (`worktree` CLI), its build directory the worktree's own `target/`.
  The paths are added to the table below as each is created.

## Commits this wave makes

This page; one design commit and one or more code commits for U1; one or more code commits for U3;
the merge of each unit into `wave/2026-11-26`; one closing planning commit with the evidence and the
moves; the merges of `main` the strict check asks for; one pull request to `main`. No tag and no
release.

## State

| unit | branch | head | worktree | stage |
|---|---|---|---|---|
| U1 | | | | planned |
| U3 | | | | planned |
