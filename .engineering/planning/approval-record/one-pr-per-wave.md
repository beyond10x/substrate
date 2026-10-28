---
format: aep.planning-md/3
id: approval-record:one-pr-per-wave
kind: approval-record
status: draft
title: One pull request to main per wave (D169)
relations:
- decides: story:adapter-stdin-pipe
- decides: story:adapter-live-output
- decides: story:host-profile-name-is-a-deployment-parameter
- decides: story:pod-force-deleted-keeps-its-policy
revision: 2
---
# Decision: one pull request to main per wave

Taken 2026-10-28 by the coordinating session as its D169, under the operator's standing grant,
relayed to this repository's session.

## Why

On 2026-10-28 this repository merged nine separate pull requests to `main` (181, 183 to 189, 191).
Each paid every required and path-triggered workflow again: the full gate, the chart lane, the
macOS host crate, documentation and the shared source gates.

## Rule

- Each unit branch of a wave merges into the wave's integration branch, as the wave protocol
  describes; it does not open its own pull request to `main`.
- Planning-store hygiene, pins and small fixes ride along in the same integration branch.
- One pull request goes from the integration branch to `main` once the wave's gate is green, and
  one release follows.
- Exception: a fix a consumer is blocked on today may go alone; its pull request body names the
  blocked consumer.

## Applied now

- `story:adapter-stdin-pipe` and `story:adapter-live-output` share the integration branch
  `wave/adapter-io`, one pull request and one release. The consumer waits on both, so one release
  serves it. This replaces the release-after-each line of `approval-record:adapter-io-waves`.
- Pull request 192 (gate wall time) was opened and its checks started before this decision; it
  merges alone because its run is already paid and it shortens every later gate run.
- The planning-hygiene branch `store/hygiene-2026-10-28c` rides in `wave/adapter-io`.

## Extended by the operator, 2026-10-28

Relayed verbatim by the coordinating session:

> Tell the substrate session to work in waves or batches. So we fix multiple stories against one
> integration branch, bigger batches, multiple stories, and then we do one PR. We do not run
> expensive full gates after every small change. We run targeted tests only for what we're
> currently implementing, and only in the end, when everything ended up in the integration
> branch, we run a local full gate and then we create the PR and auto merge it to the default
> main branch.

What it means here:

1. A wave takes every ready story whose surface does not collide, up to the wave cap of four
   units.
2. While implementing, a unit runs only its targeted tests: the crates and test targets it
   touches, plus its own specification scenarios. No full gate and no pull request per unit.
3. Unit branches merge into the integration branch locally.
4. Once every unit is in, one local full gate runs on the integration branch. On green, one pull
   request goes to `main` through the bot with auto-merge set, then one release.
5. CI on that pull request is the only remote full run.

## Units of `wave/adapter-io`

| unit | story | why it is in |
|---|---|---|
| 1 | `story:adapter-stdin-pipe` | active; design 32 |
| 2 | `story:adapter-live-output` | design 32; same surface as unit 1, so it runs after it |
| 3 | `story:host-profile-name-is-a-deployment-parameter` | D167, design first; touches the AppArmor policy, the installer, the daemon bootstrap and the chart, none of which units 1 and 2 touch |
| 4 | `story:pod-force-deleted-keeps-its-policy` | a fake-API case in the pod adapter; touches `pod.rs` only where unit 1 adds the shared stream check, so it merges after unit 1 |

`story:remote-clean-room-conformance` is not in it: design 31 is proposed with three operator
questions open, and invariant 8 allows no code before acceptance.
