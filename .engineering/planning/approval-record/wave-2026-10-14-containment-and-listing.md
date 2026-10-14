---
format: aep.planning-md/3
id: approval-record:wave-2026-10-14-containment-and-listing
kind: approval-record
status: draft
title: 'Wave 2026-10-14: session containment, cgroup teardown, directory listing'
tags:
- wave
relations:
- decides: story:session-containment-retries-an-unproven-kill
- decides: story:the-daemons-detached-containment-task-is-tracked
- decides: story:runtime-vector-cgroup-teardown-is-flaky
- decides: story:directory-listing-survives-special-entries
revision: 1
---
# Wave 2026-10-14: session containment, cgroup teardown, directory listing

Integration branch `wave/2026-10-14-containment-and-listing`, base `d9841ec6`; one pull request,
then a release.

Width: 3 units (4 stories) below the 6-agent limit, because every other draft story needs an ADR, a
successor wire bundle or a provisioned runner before code (`story:read-only-roots-pinned-against-symlinks`,
`story:transport-admission-and-stream-lifetime-disagree`, `story:session-start-publishes-exec-fields`,
`story:exec-in-an-adopted-directory` and the rest), and `story:daemon-image-serves-exec-or-says-it-cannot`
is documentation only and was left out on the scope critic's round-1 finding.

| unit | stories | tree branch | lands on |
|---|---|---|---|
| u1 | story:session-containment-retries-an-unproven-kill, then story:the-daemons-detached-containment-task-is-tracked | `unit/2026-10-14-u1-containment` | `crates/substrate-daemon/src/app/{sessions,service}.rs`, `crates/substrate-daemon/src/runtime.rs`, `crates/substrate-daemon/tests/pipe_session.rs` |
| u2 | story:runtime-vector-cgroup-teardown-is-flaky | `unit/2026-10-14-u2-cgroup` | `crates/substrate-daemon/tests/runtime_vectors.rs`, maybe `scripts/delegated-lane.sh` |
| u3 | story:directory-listing-survives-special-entries | `unit/2026-10-14-u3-dirlist` | `crates/substrate-host/src/fs.rs`, `website/docs/reference/contract.md` |

Reviews beside the build: an adversary and a security review on u1 (confinement: an unproven kill
is retried) and an adversary on u3 (a refusal becomes an omission). `CHANGELOG.md` is written by the
integrator. Plan critics: rounds 1 and 2 are the `review-result:wave-2026-10-14-*` records; the
round-2 design findings on u1 were answered by moving the retry into its own containment task
outside the `lease_sweep` guard, with a drop rule, and keeping the first attempt immediate.
